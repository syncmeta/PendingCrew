//! Durable internal causal records. No public message field confers ancestry.
use crate::{
    CollabError, ProgressKind, Result, Tx,
    cursor_reservations::{Lease, Snapshot, Window},
    messages,
};
use crewd_proto::{Message, SenderKind};
use crewd_store::StoreError;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::{BTreeMap, BTreeSet};

const SESSION: &str = "causal.session.v1.";
const MESSAGE: &str = "causal.message.v1.";
const OUTPUT: &str = "causal.output.v1.";
const HOLD: &str = "causal.hold.v1.";
const RECOVERY: &str = "causal.recovery.v1.";
const RECOVERY_ORDER: &str = "causal.recovery-order.v1";
fn invalid(s: impl Into<String>) -> CollabError {
    StoreError::Invalid(s.into()).into()
}
fn nonempty(s: &str) -> Result<()> {
    if s.trim().is_empty() {
        Err(invalid("empty causal identity"))
    } else {
        Ok(())
    }
}
fn uuid(s: &str) -> Result<()> {
    uuid::Uuid::parse_str(s)
        .map(|_| ())
        .map_err(|_| invalid("invalid causal UUID"))
}
fn next(n: u64) -> Result<u64> {
    n.checked_add(1)
        .ok_or_else(|| invalid("causal counter overflow"))
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Root {
    Event {
        session: String,
        incarnation: String,
        output: String,
    },
    Message {
        id: String,
        seq: i64,
    },
    Producer {
        session: String,
        incarnation: String,
    },
    Work {
        session: String,
        epoch: u64,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResetIdentity {
    Initial,
    Work { session: String, epoch: u64 },
    Recovery { action: String },
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainKey {
    root: Root,
    reset: ResetIdentity,
}
impl ChainKey {
    pub fn stable_key(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("typed identity serializes");
        format!(
            "c1:{}",
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
        )
    }
    fn validate(&self) -> Result<()> {
        match &self.root {
            Root::Event {
                session,
                incarnation,
                output,
            } => {
                nonempty(session)?;
                uuid(incarnation)?;
                uuid(output)?;
            }
            Root::Message { id, seq } => {
                nonempty(id)?;
                if *seq <= 0 {
                    return Err(invalid("bad root sequence"));
                }
            }
            Root::Producer {
                session,
                incarnation,
            } => {
                nonempty(session)?;
                uuid(incarnation)?;
            }
            Root::Work { session, epoch } => {
                nonempty(session)?;
                if *epoch == 0 {
                    return Err(invalid("zero work epoch"));
                }
            }
        }
        match &self.reset {
            ResetIdentity::Initial => {}
            ResetIdentity::Work { session, epoch } => {
                nonempty(session)?;
                if *epoch == 0 {
                    return Err(invalid("zero reset epoch"));
                }
            }
            ResetIdentity::Recovery { action } => uuid(action)?,
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Component {
    key: ChainKey,
    hop: u32,
}
impl Component {
    pub fn chain_id(&self) -> String {
        self.key.stable_key()
    }
    pub fn hop(&self) -> u32 {
        self.hop
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Frontier(Vec<Component>);
impl Frontier {
    pub fn components(&self) -> &[Component] {
        &self.0
    }
    fn validate(&self) -> Result<()> {
        let mut previous = None;
        for c in &self.0 {
            c.key.validate()?;
            if previous.is_some_and(|p: &ChainKey| p >= &c.key) {
                return Err(invalid("noncanonical causal frontier"));
            }
            previous = Some(&c.key);
        }
        Ok(())
    }
    fn merge(values: impl IntoIterator<Item = Frontier>) -> Self {
        let mut m = BTreeMap::<ChainKey, u32>::new();
        for f in values {
            for c in f.0 {
                m.entry(c.key)
                    .and_modify(|v| *v = (*v).max(c.hop))
                    .or_insert(c.hop);
            }
        }
        Self(
            m.into_iter()
                .map(|(key, hop)| Component { key, hop })
                .collect(),
        )
    }
    fn successor(&self) -> Result<Self> {
        self.0
            .iter()
            .map(|c| {
                Ok(Component {
                    key: c.key.clone(),
                    hop: c
                        .hop
                        .checked_add(1)
                        .ok_or_else(|| invalid("causal hop overflow"))?,
                })
            })
            .collect::<Result<Vec<_>>>()
            .map(Self)
    }
    fn reset(&self, session: &str, epoch: u64) -> Self {
        let root = Root::Work {
            session: session.into(),
            epoch,
        };
        if self.0.is_empty() {
            return Self(vec![Component {
                key: ChainKey {
                    root,
                    reset: ResetIdentity::Work {
                        session: session.into(),
                        epoch,
                    },
                },
                hop: 0,
            }]);
        }
        Self::merge(self.0.iter().map(|c| {
            Self(vec![Component {
                key: ChainKey {
                    root: c.key.root.clone(),
                    reset: ResetIdentity::Work {
                        session: session.into(),
                        epoch,
                    },
                },
                hop: 0,
            }])
        }))
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Tuple {
    id: String,
    channel: String,
    seq: i64,
    kind: SenderKind,
    sender: Option<String>,
}
impl From<&Message> for Tuple {
    fn from(m: &Message) -> Self {
        Self {
            id: m.id.clone(),
            channel: m.channel_id.clone(),
            seq: m.seq,
            kind: m.sender_kind,
            sender: m.sender_session_id.clone(),
        }
    }
}
impl Tuple {
    fn validate(&self) -> Result<()> {
        nonempty(&self.id)?;
        nonempty(&self.channel)?;
        if self.seq <= 0 {
            return Err(invalid("bad causal message seq"));
        }
        match self.kind {
            SenderKind::Session | SenderKind::Captain => {
                nonempty(self.sender.as_deref().unwrap_or(""))?
            }
            _ => {
                if self.sender.is_some() {
                    return Err(invalid("unexpected causal sender"));
                }
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionState {
    session: String,
    incarnation: String,
    epoch: u64,
    applied: u64,
    ordinal: u64,
    basis: Frontier,
    last_output: Option<String>,
    provisional: Option<Provisional>,
    active: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Provisional {
    output: String,
    epoch: u64,
    basis: Frontier,
}
fn state(c: &Connection, session: &str) -> Result<SessionState> {
    nonempty(session)?;
    let s =
        read::<SessionState>(c, &format!("{SESSION}{session}"), "session")?.unwrap_or_else(|| {
            SessionState {
                session: session.into(),
                incarnation: uuid::Uuid::new_v4().to_string(),
                epoch: 0,
                applied: 0,
                ordinal: 0,
                basis: Frontier::default(),
                last_output: None,
                provisional: None,
                active: None,
            }
        });
    if s.session != session || s.applied > s.epoch {
        return Err(invalid("invalid causal session identity/epoch"));
    }
    uuid(&s.incarnation)?;
    validate_event_frontier(c, &s.basis, None)?;
    if let Some(id) = &s.last_output {
        uuid(id)?;
    }
    if let Some(p) = &s.provisional {
        uuid(&p.output)?;
        p.basis.validate()?;
        if p.epoch > s.epoch {
            return Err(invalid("future provisional epoch"));
        }
    }
    if let Some(a) = &s.active {
        uuid(a)?;
    }
    Ok(s)
}
fn save_state(tx: &mut Tx<'_>, s: &SessionState) -> Result<()> {
    put(tx.conn(), &format!("{SESSION}{}", s.session), "session", s)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<T> {
    version: u32,
    kind: String,
    value: T,
}
fn read<T: DeserializeOwned>(c: &Connection, key: &str, kind: &str) -> Result<Option<T>> {
    let Some(raw) = crewd_store::meta_get(c, key)? else {
        return Ok(None);
    };
    let e: Envelope<T> =
        serde_json::from_str(&raw).map_err(|_| invalid(format!("invalid causal record {key}")))?;
    if e.version != 1 || e.kind != kind {
        return Err(invalid("unsupported causal envelope"));
    }
    Ok(Some(e.value))
}
fn encoded<T: Serialize>(kind: &str, value: &T) -> Result<String> {
    Ok(serde_json::to_string(&Envelope {
        version: 1,
        kind: kind.to_string(),
        value,
    })?)
}
fn put<T: Serialize>(c: &Connection, key: &str, kind: &str, value: &T) -> Result<()> {
    crewd_store::meta_set(c, key, &encoded(kind, value)?)?;
    Ok(())
}
fn insert<T: Serialize>(c: &Connection, key: &str, kind: &str, value: &T) -> Result<()> {
    c.execute(
        "INSERT INTO meta(key,value) VALUES(?1,?2)",
        params![key, encoded(kind, value)?],
    )?;
    Ok(())
}
fn keys(c: &Connection, prefix: &str) -> Result<Vec<String>> {
    let mut q = c.prepare("SELECT key FROM meta WHERE substr(key,1,length(?1))=?1 ORDER BY key")?;
    let rows = q
        .query_map([prefix], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Capture {
    session: String,
    epoch: u64,
    applied: u64,
    // Exact canonical output identity at capture, independent of Work epoch.
    // Old envelopes omit this field; they remain conservative after a known newer output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_output: Option<String>,
    tuples: Vec<Tuple>,
    parts: Vec<Frontier>,
    windows: Vec<Window>,
    unresolved_ids: Vec<String>,
    frontier: Frontier,
    unresolved: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CausalSnapshot(Capture);
impl CausalSnapshot {
    pub fn frontier(&self) -> &Frontier {
        &self.0.frontier
    }
    pub fn unresolved(&self) -> bool {
        self.0.unresolved
    }
    pub fn select(&self, targets: &[(String, i64)]) -> Result<Self> {
        let mut seen = BTreeSet::new();
        let mut windows = Vec::new();
        for (ch, end) in targets {
            let w = self
                .0
                .windows
                .iter()
                .find(|w| w.channel == *ch)
                .ok_or_else(|| invalid("uncaptured causal channel"))?;
            if !seen.insert(ch)
                || *end <= w.from_exclusive
                || *end > w.to_inclusive
                || !self
                    .0
                    .tuples
                    .iter()
                    .any(|t| t.channel == *ch && t.seq == *end)
            {
                return Err(invalid("not a captured causal prefix end"));
            }
            windows.push(Window {
                channel: ch.clone(),
                from_exclusive: w.from_exclusive,
                to_inclusive: *end,
            });
        }
        let mut c = self.0.clone();
        c.windows = windows;
        c.tuples.clear();
        c.parts.clear();
        for (t, f) in self.0.tuples.iter().zip(&self.0.parts) {
            if c.windows.iter().any(|w| {
                t.channel == w.channel && t.seq > w.from_exclusive && t.seq <= w.to_inclusive
            }) {
                c.tuples.push(t.clone());
                c.parts.push(f.clone());
            }
        }
        c.unresolved_ids
            .retain(|id| c.tuples.iter().any(|t| t.id == *id));
        c.unresolved = !c.unresolved_ids.is_empty();
        c.frontier = Frontier::merge(c.parts.clone());
        Ok(Self(c))
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum ReceiptOwner {
    Executor { generation: u64 },
    Hook { operation: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EventIdentity {
    id: i64,
    kind: crate::wakes::WakeKind,
    source: String,
    created_at: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    id: String,
    session: String,
    incarnation: String,
    token: String,
    owner: ReceiptOwner,
    capture: Capture,
    events: Vec<EventIdentity>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingOutput(Receipt);
impl PendingOutput {
    pub fn stable_key(&self) -> &str {
        &self.0.id
    }
    pub fn session(&self) -> &str {
        &self.0.session
    }
    pub fn generation(&self) -> u64 {
        match &self.0.owner {
            ReceiptOwner::Executor { generation } => *generation,
            ReceiptOwner::Hook { .. } => {
                unreachable!("hook output has no public PendingOutput capability")
            }
        }
    }
    /// Trusted producer adapter only: call at accepted Delivered event emission, not BodyWritten.
    pub fn observe_success(
        &self,
        delivery_id: u64,
        generation: u64,
        producer: &str,
        serial: u64,
    ) -> Result<ObservedSuccess> {
        uuid(producer)?;
        if !matches!(&self.0.owner,ReceiptOwner::Executor{generation:g} if *g==generation)
            || serial == 0
        {
            return Err(invalid("success fence ownership mismatch"));
        }
        Ok(ObservedSuccess(Success {
            receipt: self.0.clone(),
            kind: SuccessKind::Executor {
                delivery_id,
                producer: producer.into(),
                serial,
            },
        }))
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum SuccessKind {
    Executor {
        delivery_id: u64,
        producer: String,
        serial: u64,
    },
    Hook {
        operation: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Success {
    receipt: Receipt,
    kind: SuccessKind,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedSuccess(Success);
impl ObservedSuccess {
    pub fn pending(&self) -> PendingOutput {
        PendingOutput(self.0.receipt.clone())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostObservation {
    producer: String,
    ordinal: u64,
    candidate: Option<PendingOutput>,
    success: Option<ObservedSuccess>,
}
impl PostObservation {
    pub fn new(
        producer: &str,
        ordinal: u64,
        candidate: Option<PendingOutput>,
        success: Option<ObservedSuccess>,
    ) -> Result<Self> {
        uuid(producer)?;
        if ordinal==0||success.as_ref().is_some_and(|s|!matches!(&s.0.kind,SuccessKind::Executor{producer:p,serial,..} if p==producer && *serial<=ordinal)){return Err(invalid("unordered success fence"));}
        Ok(Self {
            producer: producer.into(),
            ordinal,
            candidate,
            success,
        })
    }
}
pub trait PostObservationSource: Send + Sync {
    fn observe(&self, session: &str) -> Result<PostObservation>;
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimedOutput {
    pub lease: Lease,
    pub pending: PendingOutput,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputCommit {
    pub newly_committed: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookCommit {
    pub output: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OutputStatus {
    Pending,
    Committed,
    UnresolvedTerminal,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputRecord {
    receipt: Receipt,
    status: OutputStatus,
    success: Option<Success>,
}
fn validate_receipt(r: &Receipt) -> Result<()> {
    uuid(&r.id)?;
    nonempty(&r.session)?;
    uuid(&r.incarnation)?;
    uuid(&r.token)?;
    match &r.owner {
        ReceiptOwner::Executor { generation } => {
            if *generation == 0 {
                return Err(invalid("zero output generation"));
            }
        }
        ReceiptOwner::Hook { operation } => uuid(operation)?,
    }
    if r.capture.session != r.session || r.capture.applied > r.capture.epoch {
        return Err(invalid("invalid causal receipt"));
    }
    validate_capture(&r.capture)?;
    let mut previous = 0;
    for event in &r.events {
        if event.id <= previous
            || !matches!(
                event.kind,
                crate::wakes::WakeKind::Timer
                    | crate::wakes::WakeKind::AskAnswer
                    | crate::wakes::WakeKind::System
            )
        {
            return Err(invalid("invalid original autonomous event identity"));
        }
        nonempty(&event.source)?;
        nonempty(&event.created_at)?;
        previous = event.id;
    }
    if !r.events.is_empty()
        && (!r.capture.tuples.is_empty() || !matches!(r.owner, ReceiptOwner::Executor { .. }))
    {
        return Err(invalid("autonomous event on message/hook receipt"));
    }
    Ok(())
}
fn validate_capture(c: &Capture) -> Result<()> {
    nonempty(&c.session)?;
    if let Some(id) = &c.previous_output {
        uuid(id)?;
    }
    if c.applied > c.epoch {
        return Err(invalid("future applied capture epoch"));
    }
    c.frontier.validate()?;
    if c.parts.len() != c.tuples.len()
        || c.frontier != Frontier::merge(c.parts.clone())
        || c.unresolved == c.unresolved_ids.is_empty()
    {
        return Err(invalid("invalid causal capture components"));
    }
    let mut channels = BTreeSet::new();
    for w in &c.windows {
        nonempty(&w.channel)?;
        if w.from_exclusive < 0
            || w.to_inclusive <= w.from_exclusive
            || !channels.insert(&w.channel)
        {
            return Err(invalid("invalid causal capture window"));
        }
    }
    let mut ids = BTreeSet::new();
    let mut latest = BTreeMap::new();
    for (t, f) in c.tuples.iter().zip(&c.parts) {
        t.validate()?;
        f.validate()?;
        let w = c
            .windows
            .iter()
            .find(|w| w.channel == t.channel)
            .ok_or_else(|| invalid("captured tuple outside windows"))?;
        if !ids.insert(&t.id)
            || t.seq <= w.from_exclusive
            || t.seq > w.to_inclusive
            || latest.get(&t.channel).is_some_and(|n| *n >= t.seq)
        {
            return Err(invalid("duplicate/out-of-prefix causal tuple"));
        }
        latest.insert(&t.channel, t.seq);
    }
    for w in &c.windows {
        if latest.get(&w.channel) != Some(&w.to_inclusive) {
            return Err(invalid("captured prefix end missing"));
        }
    }
    let mut unknown = BTreeSet::new();
    for id in &c.unresolved_ids {
        if !ids.contains(id) || !unknown.insert(id) {
            return Err(invalid("invalid unresolved capture tuple"));
        }
    }
    Ok(())
}
fn validate_success(s: &Success) -> Result<()> {
    match (&s.receipt.owner, &s.kind) {
        (
            ReceiptOwner::Executor { generation },
            SuccessKind::Executor {
                producer, serial, ..
            },
        ) => {
            uuid(producer)?;
            if *generation == 0 || *serial == 0 {
                return Err(invalid("invalid success evidence"));
            }
        }
        (ReceiptOwner::Hook { operation: a }, SuccessKind::Hook { operation: b }) if a == b => {
            uuid(a)?
        }
        _ => return Err(invalid("output/success kind mismatch")),
    }
    Ok(())
}
fn output(c: &Connection, id: &str) -> Result<OutputRecord> {
    uuid(id)?;
    let o = read::<OutputRecord>(c, &format!("{OUTPUT}{id}"), "output")?
        .ok_or_else(|| invalid("causal output missing"))?;
    validate_receipt(&o.receipt)?;
    let ids = o
        .receipt
        .events
        .iter()
        .map(|event| event.id)
        .collect::<Vec<_>>();
    for row in crate::wakes::by_ids(c, &ids)? {
        let event = o
            .receipt
            .events
            .iter()
            .find(|event| event.id == row.id)
            .ok_or_else(|| invalid("unexpected original event row"))?;
        if row.session != o.receipt.session
            || row.kind != event.kind
            || row.source != event.source
            || row.created_at != event.created_at
        {
            return Err(invalid(
                "original event evidence differs from retained actual row",
            ));
        }
    }
    // Normal retention/purge may remove original rows and source sessions. Immutable evidence
    // remains in this exact receipt; their absence is not an invitation to invent another event.
    if o.receipt.id != id {
        return Err(invalid("output key mismatch"));
    }
    if let Some(s) = &o.success {
        validate_success(s)?;
        if s.receipt != o.receipt || o.status != OutputStatus::Committed {
            return Err(invalid("invalid successful output"));
        }
    }
    if o.status == OutputStatus::Committed && o.success.is_none() {
        return Err(invalid("success evidence missing"));
    }
    Ok(o)
}
fn validate_event_root(c: &Connection, key: &ChainKey, observed: Option<&Success>) -> Result<()> {
    let Root::Event {
        session,
        incarnation,
        output: id,
    } = &key.root
    else {
        return Ok(());
    };
    let original = output(c, id)?;
    if original.receipt.session != *session
        || original.receipt.incarnation != *incarnation
        || original.receipt.events.is_empty()
        || !original.receipt.capture.tuples.is_empty()
        || !matches!(&original.receipt.owner, ReceiptOwner::Executor { .. })
    {
        return Err(invalid(
            "event root does not identify its original autonomous output",
        ));
    }
    if original.status == OutputStatus::Committed {
        // output() already validates exact receipt equality, kind and known-success evidence.
        return Ok(());
    }
    if original.status == OutputStatus::Pending
        && let Some(success) = observed
    {
        validate_success(success)?;
        if success.receipt == original.receipt {
            return Ok(());
        }
    }
    Err(invalid(
        "event root has no exact successful output evidence",
    ))
}
fn validate_event_frontier(c: &Connection, f: &Frontier, observed: Option<&Success>) -> Result<()> {
    f.validate()?;
    for component in &f.0 {
        validate_event_root(c, &component.key, observed)?;
    }
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Resolution {
    Resolved {
        frontier: Frontier,
    },
    Eligible {
        output: String,
        frontier: Frontier,
        success: Box<Success>,
    },
    Unresolved {
        previous: Frontier,
        candidate: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stamp {
    tuple: Tuple,
    source: Option<String>,
    work_epoch: u64,
    post_ordinal: u64,
    candidate: Option<String>,
    resolution: Resolution,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageCause {
    Resolved(Frontier),
    Unresolved { previous: Frontier },
}
pub fn read_message(c: &Connection, m: &Message) -> Result<MessageCause> {
    let Some(s) = read::<Stamp>(c, &format!("{MESSAGE}{}", m.id), "message")? else {
        return Ok(MessageCause::Resolved(Frontier(vec![Component {
            key: ChainKey {
                root: Root::Message {
                    id: m.id.clone(),
                    seq: m.seq,
                },
                reset: ResetIdentity::Initial,
            },
            hop: 0,
        }])));
    };
    s.tuple.validate()?;
    if s.tuple != Tuple::from(m) {
        return Err(invalid("causal message tuple mismatch"));
    }
    if let Some(src) = &s.source {
        nonempty(src)?;
        if matches!(m.sender_kind, SenderKind::Session | SenderKind::Captain)
            && m.sender_session_id.as_deref() != Some(src.as_str())
        {
            return Err(invalid("causal source mismatch"));
        }
    }
    if let Some(id) = &s.candidate {
        uuid(id)?;
    }
    match s.resolution {
        Resolution::Resolved { frontier } => {
            validate_event_frontier(c, &frontier, None)?;
            Ok(MessageCause::Resolved(frontier))
        }
        Resolution::Eligible {
            output: id,
            frontier,
            success,
        } => {
            validate_success(success.as_ref())?;
            validate_event_frontier(c, &frontier, Some(success.as_ref()))?;
            let o = output(c, &id)?;
            if o.receipt != success.receipt {
                return Err(invalid("eligible parent mismatch"));
            }
            if o.status == OutputStatus::Committed && o.success.as_ref() == Some(success.as_ref()) {
                Ok(MessageCause::Resolved(frontier))
            } else {
                Ok(MessageCause::Unresolved { previous: frontier })
            }
        }
        Resolution::Unresolved {
            previous,
            candidate,
        } => {
            validate_event_frontier(c, &previous, None)?;
            output(c, &candidate)?;
            Ok(MessageCause::Unresolved { previous })
        }
    }
}
pub fn capture(
    c: &Connection,
    snap: &Snapshot,
    targets: &[(String, i64)],
) -> Result<CausalSnapshot> {
    let session = snap.causal_session();
    let st = state(c, session)?;
    let windows = snap.windows_for(targets)?;
    let mut tuples = Vec::new();
    let mut parts = Vec::new();
    let mut unresolved_ids = Vec::new();
    for w in &windows {
        let (ms, _) = crate::cursors::scan_unread(c, session, &w.channel, snap.automatic_after(w.from_exclusive))?;
        for m in ms.into_iter().filter(|m| m.seq <= w.to_inclusive) {
            let f = match read_message(c, &m)? {
                MessageCause::Resolved(f) => recovered(c, session, &m, f)?,
                MessageCause::Unresolved { previous } => {
                    let (f, ok) = recover_unresolved(c, session, &m, previous)?;
                    if !ok {
                        unresolved_ids.push(m.id.clone());
                    }
                    f
                }
            };
            parts.push(f);
            tuples.push(Tuple::from(&m));
        }
    }
    let frontier = Frontier::merge(parts.clone());
    let unresolved = !unresolved_ids.is_empty();
    Ok(CausalSnapshot(Capture {
        session: session.into(),
        epoch: st.epoch,
        applied: st.applied,
        previous_output: st.last_output,
        tuples,
        parts,
        windows,
        unresolved_ids,
        frontier,
        unresolved,
    }))
}

/// Internal causal policy, not a user-configurable throughput limit.
pub const CAUSAL_HOP_MAX: u32 = 16;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Admission {
    pub channels: Vec<ChannelAdmission>,
    /// A missing persistent cursor cannot establish an unread prefix. No lazy writes here.
    pub unknown_channels: Vec<String>,
}
impl Admission {
    pub fn blocked(&self) -> bool {
        self.channels.iter().any(|ch| ch.blocked)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelAdmission {
    pub channel: String,
    pub blocked: bool,
    pub holds: Vec<ChainHold>,
    pub proposed_holds: Vec<ChainHold>,
    pub unresolved: Vec<UnresolvedCause>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnresolvedCause {
    pub message_id: String,
    pub seq: i64,
    pub parent_receipt: String,
    pub source: String,
    pub target: String,
    pub diagnostic_chain_id: String,
}
fn diagnostic_key(m: &Message) -> ChainKey {
    ChainKey {
        root: Root::Message {
            id: m.id.clone(),
            seq: m.seq,
        },
        reset: ResetIdentity::Initial,
    }
}
fn trusted_source(c: &Connection, m: &Message) -> Result<Option<String>> {
    // This loads strict ancestry too; a corrupt present marker is never legacy absence.
    read_message(c, m)?;
    let stamp = read::<Stamp>(c, &format!("{MESSAGE}{}", m.id), "message")?;
    match m.sender_kind {
        SenderKind::Human => Ok(None),
        SenderKind::Session | SenderKind::Captain => {
            let source = m
                .sender_session_id
                .clone()
                .ok_or_else(|| invalid("actual sender missing"))?;
            nonempty(&source)?;
            if stamp
                .as_ref()
                .is_some_and(|s| s.source.as_deref() != Some(source.as_str()))
            {
                return Err(invalid("trusted causal sender mismatch"));
            }
            Ok(Some(source))
        }
        SenderKind::System => {
            let source = stamp
                .and_then(|s| s.source)
                .unwrap_or_else(|| "system".into());
            nonempty(&source)?;
            Ok(Some(source))
        }
    }
}
fn unresolved_parent(c: &Connection, m: &Message) -> Result<Option<String>> {
    let Some(stamp) = read::<Stamp>(c, &format!("{MESSAGE}{}", m.id), "message")? else {
        return Ok(None);
    };
    let candidate = match stamp.resolution {
        Resolution::Unresolved { candidate, .. } => Some(candidate),
        Resolution::Eligible {
            output: id,
            success,
            ..
        } => {
            let record = output(c, &id)?;
            if record.status == OutputStatus::Committed
                && record.success.as_ref() == Some(success.as_ref())
            {
                None
            } else {
                Some(id)
            }
        }
        Resolution::Resolved { .. } => None,
    };
    if let Some(id) = &candidate {
        output(c, id)?;
    }
    Ok(candidate)
}
fn effective_message(c: &Connection, target: &str, m: &Message) -> Result<(Frontier, bool)> {
    match read_message(c, m)? {
        MessageCause::Resolved(f) => Ok((recovered(c, target, m, f)?, false)),
        MessageCause::Unresolved { previous } => {
            let (f, resolved) = recover_unresolved(c, target, m, previous)?;
            Ok((f, !resolved))
        }
    }
}
pub fn inspect_admission(
    c: &Connection,
    snapshot: &CausalSnapshot,
    max_hops: u32,
    now_ms: i64,
) -> Result<Admission> {
    if now_ms < 0 {
        return Err(invalid("negative admission time"));
    }
    validate_capture(&snapshot.0)?;
    let target = &snapshot.0.session;
    let existing = holds(c, Some(target))?;
    let mut channels = snapshot
        .0
        .windows
        .iter()
        .map(|w| ChannelAdmission {
            channel: w.channel.clone(),
            blocked: false,
            holds: Vec::new(),
            proposed_holds: Vec::new(),
            unresolved: Vec::new(),
        })
        .collect::<Vec<_>>();
    for ((tuple, part), was_unknown) in
        snapshot.0.tuples.iter().zip(&snapshot.0.parts).map(|pair| {
            let unknown = snapshot.0.unresolved_ids.contains(&pair.0.id);
            (pair, unknown)
        })
    {
        let m = messages::get(c, &tuple.id)?;
        if Tuple::from(&m) != *tuple {
            return Err(invalid("admission actual tuple mismatch"));
        }
        let (effective, unknown) = effective_message(c, target, &m)?;
        if effective != *part || unknown != was_unknown {
            return Err(invalid("admission effective capture changed"));
        }
        let Some(source) = trusted_source(c, &m)? else {
            continue;
        }; // Only this direct actual Human row is immune.
        let ch = channels
            .iter_mut()
            .find(|ch| ch.channel == m.channel_id)
            .ok_or_else(|| invalid("admission channel not captured"))?;
        let mut required = Vec::new();
        for component in effective.components() {
            let chain_id = component.chain_id();
            if let Some(h) = existing
                .iter()
                .find(|h| h.chain_id == chain_id && h.from == source)
            {
                required.push(h.clone());
            } else if component.hop() > max_hops {
                required.push(ChainHold {
                    chain_id,
                    from: source.clone(),
                    target: target.clone(),
                    hop: component.hop(),
                    reason: "causal hop limit".into(),
                    at_ms: now_ms,
                });
            }
        }
        if unknown {
            let parent_receipt = unresolved_parent(c, &m)?
                .ok_or_else(|| invalid("unresolved diagnostic has no exact parent"))?;
            let diagnostic_chain_id = diagnostic_key(&m).stable_key();
            ch.unresolved.push(UnresolvedCause {
                message_id: m.id.clone(),
                seq: m.seq,
                parent_receipt,
                source: source.clone(),
                target: target.clone(),
                diagnostic_chain_id: diagnostic_chain_id.clone(),
            });
            required.push(
                existing
                    .iter()
                    .find(|h| h.chain_id == diagnostic_chain_id && h.from == source)
                    .cloned()
                    .unwrap_or(ChainHold {
                        chain_id: diagnostic_chain_id,
                        from: source,
                        target: target.clone(),
                        hop: 0,
                        reason: "causal unresolved".into(),
                        at_ms: now_ms,
                    }),
            );
        }
        for h in required {
            let dest = if existing.iter().any(|old| {
                old.chain_id == h.chain_id && old.from == h.from && old.target == h.target
            }) {
                &mut ch.holds
            } else {
                &mut ch.proposed_holds
            };
            if !dest.iter().any(|old| {
                old.chain_id == h.chain_id && old.from == h.from && old.target == h.target
            }) {
                dest.push(h);
            }
        }
        ch.blocked =
            !ch.holds.is_empty() || !ch.proposed_holds.is_empty() || !ch.unresolved.is_empty();
    }
    Ok(Admission {
        channels,
        unknown_channels: Vec::new(),
    })
}
/// Pure read. Never prepare_unread, lazy-pin or advance a leading-own cursor.
pub fn diagnostics(c: &Connection, session: &str, now_ms: i64) -> Result<Admission> {
    diagnostics_after(c,session,now_ms,None)
}
pub fn diagnostics_after(c:&Connection,session:&str,now_ms:i64,after:Option<i64>)->Result<Admission>{
    if after.is_some_and(|n|n<0){return Err(invalid("negative automatic message floor"))}
    let reader = crate::sessions::get(c, session)?;
    if crate::crews::is_archived(c, &reader.crew_id)? {
        return Ok(Admission {
            channels: Vec::new(),
            unknown_channels: Vec::new(),
        });
    }
    let mut channels = Vec::new();
    let mut ends = Vec::new();
    let mut unknown_channels = Vec::new();
    for channel in crate::cursors::injection_channels(c, session, now_ms)? {
        if crate::channels::get(c, &channel)?.archived_at.is_some() {
            continue;
        }
        let Some(base) = crate::cursors::cursor(c, session, &channel)? else {
            unknown_channels.push(channel);
            continue;
        };
        let (messages, _) = crate::cursors::scan_unread(c, session, &channel, base.max(after.unwrap_or(0)))?;
        if let Some(m) = messages.last() {
            ends.push((channel.clone(), m.seq));
            channels.push(channel);
        }
    }
    let snapshot = crate::cursor_reservations::snapshot_after(c, session, &channels, now_ms,after)?;
    let captured = capture(c, &snapshot, &ends)?;
    let mut result = inspect_admission(c, &captured, CAUSAL_HOP_MAX, now_ms)?;
    result.unknown_channels = unknown_channels;
    Ok(result)
}
pub(crate) fn initialize(tx: &mut Tx<'_>, session: &str) -> Result<()> {
    let s = state(tx.conn(), session)?;
    save_state(tx, &s)
}
pub(crate) fn note_work(tx: &mut Tx<'_>, session: &str, _kind: ProgressKind) -> Result<()> {
    let mut s = state(tx.conn(), session)?;
    s.epoch = next(s.epoch)?;
    s.provisional = None;
    save_state(tx, &s)
}
pub(crate) fn stamp_post(tx: &mut Tx<'_>, m: &Message, relay_source: Option<&str>) -> Result<()> {
    let actual = messages::get(tx.conn(), &m.id)?;
    if actual != *m {
        return Err(invalid("stamp differs from actual message"));
    }
    let source = relay_source
        .map(str::to_string)
        .or_else(|| m.sender_session_id.clone());
    if relay_source.is_some() {
        match m.sender_kind {
            SenderKind::Human => return Err(invalid("human relay source")),
            SenderKind::Session | SenderKind::Captain => {
                if m.sender_session_id.as_deref() != relay_source {
                    return Err(invalid("relay causal source mismatch"));
                }
            }
            SenderKind::System => {}
        }
    }
    let mut candidate = None;
    let (epoch, ordinal, resolution) = if let Some(ref session) = source {
        crate::sessions::get(tx.conn(), session)?;
        let mut s = state(tx.conn(), session)?;
        s.ordinal = next(s.ordinal)?;
        let observation = tx.post_observation(session)?;
        if let Some(o) = &observation {
            uuid(&o.producer)?;
            if o.ordinal == 0 {
                return Err(invalid("invalid post fence"));
            }
            if o.candidate.as_ref().is_some_and(|p| p.session() != session) {
                return Err(invalid("wrong observation session"));
            }
        }
        candidate = s.active.clone();
        if let Some(o) = &observation
            && let Some(p) = &o.candidate
        {
            let actual = output(tx.conn(), p.stable_key())?;
            if actual.receipt != p.0 {
                return Err(invalid("candidate output mismatch"));
            }
            candidate = Some(p.stable_key().into());
        }
        let success = observation.and_then(|o| o.success);
        let qualifying = if let Some(ok) = success {
            if ok.0.receipt.session != *session {
                return Err(invalid("success belongs to another session"));
            }
            let rec = output(tx.conn(), &ok.0.receipt.id)?;
            if rec.receipt != ok.0.receipt {
                return Err(invalid("success receipt mismatch"));
            }
            if rec.receipt.capture.epoch > s.epoch {
                return Err(invalid("future output work epoch"));
            }
            let empty_event_eligible = !rec.receipt.events.is_empty()
                && s.epoch == s.applied
                && (s.basis.0.is_empty()
                    || (rec.status == OutputStatus::Committed
                        && s.last_output.as_deref() == Some(rec.receipt.id.as_str())));
            if (rec.receipt.capture.tuples.is_empty() && !empty_event_eligible)
                || rec.status == OutputStatus::UnresolvedTerminal
                || (rec.receipt.capture.epoch < s.epoch
                    && (s.applied == s.epoch
                        || s.active.as_deref() != Some(rec.receipt.id.as_str())))
                || (rec.status == OutputStatus::Committed
                    && s.last_output
                        .as_deref()
                        .is_some_and(|id| id != rec.receipt.id.as_str()))
            {
                None
            } else {
                Some((ok, rec))
            }
        } else {
            None
        };
        let mut f = s.basis.clone();
        let new_work = relay_source.is_none() && s.epoch > s.applied;
        let r = if let Some((ok, rec)) = qualifying {
            let id = rec.receipt.id.clone();
            candidate = Some(id.clone());
            f = if rec.status == OutputStatus::Committed
                && s.last_output.as_deref() == Some(id.as_str())
            {
                s.basis.clone()
            } else if let Some(p) = &s.provisional {
                if p.output == id && p.epoch == s.epoch {
                    p.basis.clone()
                } else {
                    if rec.receipt.capture.tuples.is_empty() {
                        event_basis(&rec.receipt)?
                    } else {
                        rec.receipt.capture.frontier.successor()?
                    }
                }
            } else {
                if rec.receipt.capture.tuples.is_empty() {
                    event_basis(&rec.receipt)?
                } else {
                    rec.receipt.capture.frontier.successor()?
                }
            };
            if new_work {
                f = rec.receipt.capture.frontier.reset(session, s.epoch);
                s.applied = s.epoch;
            }
            if rec.status == OutputStatus::Committed {
                if rec.success.as_ref() != Some(&ok.0) {
                    return Err(invalid("conflicting success ticket"));
                }
                s.basis = f.clone();
                s.last_output = Some(id);
                Resolution::Resolved { frontier: f }
            } else {
                s.provisional = Some(Provisional {
                    output: id.clone(),
                    epoch: s.epoch,
                    basis: f.clone(),
                });
                Resolution::Eligible {
                    output: id,
                    frontier: f,
                    success: Box::new(ok.0),
                }
            }
        } else if new_work {
            f = f.reset(session, s.epoch);
            s.applied = s.epoch;
            s.basis = f.clone();
            Resolution::Resolved { frontier: f }
        } else if candidate.is_some() && f.0.is_empty() {
            Resolution::Unresolved {
                previous: f,
                candidate: candidate.clone().expect("checked"),
            }
        } else {
            if f.0.is_empty() {
                f = Frontier(vec![Component {
                    key: ChainKey {
                        root: Root::Producer {
                            session: session.clone(),
                            incarnation: s.incarnation.clone(),
                        },
                        reset: ResetIdentity::Initial,
                    },
                    hop: 0,
                }]);
                s.basis = f.clone();
            }
            Resolution::Resolved { frontier: f }
        };
        let epoch = s.epoch;
        let ordinal = s.ordinal;
        save_state(tx, &s)?;
        (epoch, ordinal, r)
    } else {
        let f = Frontier(vec![Component {
            key: ChainKey {
                root: Root::Message {
                    id: m.id.clone(),
                    seq: m.seq,
                },
                reset: ResetIdentity::Initial,
            },
            hop: 0,
        }]);
        (0, 0, Resolution::Resolved { frontier: f })
    };
    insert(
        tx.conn(),
        &format!("{MESSAGE}{}", m.id),
        "message",
        &Stamp {
            tuple: Tuple::from(m),
            source,
            work_epoch: epoch,
            post_ordinal: ordinal,
            candidate,
            resolution,
        },
    )
}
fn event_basis(r: &Receipt) -> Result<Frontier> {
    validate_receipt(r)?;
    if r.events.is_empty() {
        return Err(invalid("no original autonomous event"));
    }
    Ok(Frontier(vec![Component {
        key: ChainKey {
            root: Root::Event {
                session: r.session.clone(),
                incarnation: r.incarnation.clone(),
                output: r.id.clone(),
            },
            reset: ResetIdentity::Initial,
        },
        hop: 0,
    }]))
}
pub(crate) fn register_pending(
    tx: &mut Tx<'_>,
    session: &str,
    incarnation: &str,
    token: &str,
    generation: u64,
    causes: &CausalSnapshot,
    wake_ids: &[i64],
) -> Result<PendingOutput> {
    let mut s = state(tx.conn(), session)?;
    if s.epoch != causes.0.epoch || s.applied != causes.0.applied || causes.0.session != session {
        return Err(invalid("causal capture changed"));
    }
    if causes.0.unresolved {
        return Err(invalid("unresolved body cannot be claimed"));
    }
    for t in &causes.0.tuples {
        let m = messages::get(tx.conn(), &t.id)?;
        if Tuple::from(&m) != *t {
            return Err(invalid("causal captured tuple changed"));
        }
    }
    if let Some(id) = &s.active {
        let old = output(tx.conn(), id)?;
        if old.status == OutputStatus::Pending {
            return Err(invalid("causal owner still pending"));
        }
    }
    let mut events = Vec::new();
    if causes.0.tuples.is_empty() && !wake_ids.is_empty() {
        let mut rows = crate::wakes::by_ids(tx.conn(), wake_ids)?;
        rows.sort_by_key(|r| r.id);
        if rows.len() != wake_ids.len()
            || rows
                .iter()
                .any(|r| r.session != session || r.state != crate::wakes::WakeState::Inflight)
        {
            return Err(invalid("original event rows do not match actual claim"));
        }
        if rows.iter().all(|r| {
            matches!(
                r.kind,
                crate::wakes::WakeKind::Timer
                    | crate::wakes::WakeKind::AskAnswer
                    | crate::wakes::WakeKind::System
            )
        }) {
            events = rows
                .into_iter()
                .map(|r| EventIdentity {
                    id: r.id,
                    kind: r.kind,
                    source: r.source,
                    created_at: r.created_at,
                })
                .collect();
        }
    }
    let r = Receipt {
        id: uuid::Uuid::new_v4().to_string(),
        session: session.into(),
        incarnation: incarnation.into(),
        token: token.into(),
        owner: ReceiptOwner::Executor { generation },
        capture: causes.0.clone(),
        events,
    };
    validate_receipt(&r)?;
    insert(
        tx.conn(),
        &format!("{OUTPUT}{}", r.id),
        "output",
        &OutputRecord {
            receipt: r.clone(),
            status: OutputStatus::Pending,
            success: None,
        },
    )?;
    s.active = Some(r.id.clone());
    save_state(tx, &s)?;
    Ok(PendingOutput(r))
}
pub(crate) fn hook_pending(
    tx: &mut Tx<'_>,
    session: &str,
    causes: &CausalSnapshot,
) -> Result<PendingOutput> {
    let id = uuid::Uuid::new_v4().to_string();
    let s = state(tx.conn(), session)?;
    if causes.0.session != session || causes.0.epoch != s.epoch || causes.0.unresolved {
        return Err(invalid("hook cause changed/unresolved"));
    }
    let r = Receipt {
        id: id.clone(),
        session: session.into(),
        incarnation: s.incarnation,
        token: id.clone(),
        owner: ReceiptOwner::Hook {
            operation: id.clone(),
        },
        capture: causes.0.clone(),
        events: Vec::new(),
    };
    validate_receipt(&r)?;
    insert(
        tx.conn(),
        &format!("{OUTPUT}{id}"),
        "output",
        &OutputRecord {
            receipt: r.clone(),
            status: OutputStatus::Pending,
            success: None,
        },
    )?;
    Ok(PendingOutput(r))
}
pub(crate) fn finalize_hook(tx: &mut Tx<'_>, p: &PendingOutput) -> Result<OutputCommit> {
    let ReceiptOwner::Hook { operation } = &p.0.owner else {
        return Err(invalid("not a hook receipt"));
    };
    finalize(
        tx,
        p,
        &ObservedSuccess(Success {
            receipt: p.0.clone(),
            kind: SuccessKind::Hook {
                operation: operation.clone(),
            },
        }),
    )
}
impl CausalSnapshot {
    pub(crate) fn matches_windows(
        &self,
        c: &Connection,
        snap: &Snapshot,
        windows: &[Window],
    ) -> Result<bool> {
        if self.0.windows != windows {
            return Err(invalid("causal/window selection mismatch"));
        }
        let ends = windows
            .iter()
            .map(|w| (w.channel.clone(), w.to_inclusive))
            .collect::<Vec<_>>();
        Ok(capture(c, snap, &ends)? == *self)
    }
}
pub(crate) fn pending_for(
    c: &Connection,
    session: &str,
    incarnation: &str,
    token: &str,
    generation: u64,
) -> Result<Option<PendingOutput>> {
    let s = state(c, session)?;
    let Some(id) = s.active else { return Ok(None) };
    let o = output(c, &id)?;
    if o.receipt.incarnation == incarnation
        && o.receipt.token == token
        && matches!(o.receipt.owner,ReceiptOwner::Executor{generation:g} if g==generation)
    {
        Ok(Some(PendingOutput(o.receipt)))
    } else {
        Ok(None)
    }
}
pub(crate) fn committed(c: &Connection, p: &PendingOutput) -> Result<bool> {
    let o = output(c, p.stable_key())?;
    if o.receipt != p.0 {
        return Err(invalid("causal receipt changed"));
    }
    Ok(o.status == OutputStatus::Committed)
}
pub(crate) fn finalize(
    tx: &mut Tx<'_>,
    p: &PendingOutput,
    success: &ObservedSuccess,
) -> Result<OutputCommit> {
    if success.0.receipt != p.0 {
        return Err(invalid("wrong success receipt"));
    }
    validate_success(&success.0)?;
    let mut o = output(tx.conn(), p.stable_key())?;
    if o.receipt != p.0 {
        return Err(invalid("receipt mismatch"));
    }
    if o.status == OutputStatus::Committed {
        if o.success.as_ref() != Some(&success.0) {
            return Err(invalid("conflicting duplicate success"));
        }
        return Ok(OutputCommit {
            newly_committed: false,
        });
    }
    if o.status != OutputStatus::Pending {
        return Err(invalid("terminal output cannot become successful"));
    }
    o.status = OutputStatus::Committed;
    o.success = Some(success.0.clone());
    put(
        tx.conn(),
        &format!("{OUTPUT}{}", p.stable_key()),
        "output",
        &o,
    )?;
    let mut s = state(tx.conn(), p.session())?;
    let mut installed = false;
    if p.0.capture.tuples.is_empty() {
        // Only a true original autonomous event may establish the first basis; pending Work,
        // existing ancestry and empty brief/registration receipts retain their earlier semantics.
        if !p.0.events.is_empty()
            && s.epoch == s.applied
            && s.epoch == p.0.capture.epoch
            && s.basis.0.is_empty()
            && s.active.as_deref() == Some(p.stable_key())
        {
            s.basis = if let Some(provisional) = &s.provisional {
                if provisional.output == p.stable_key() && provisional.epoch == s.epoch {
                    provisional.basis.clone()
                } else {
                    event_basis(&p.0)?
                }
            } else {
                event_basis(&p.0)?
            };
            s.provisional = None;
            installed = true;
        }
    } else if let Some(provisional) = &s.provisional {
        if provisional.output == p.stable_key() && provisional.epoch == s.epoch {
            s.basis = provisional.basis.clone();
            s.provisional = None;
            installed = true;
        }
    } else if s.applied < s.epoch && s.active.as_deref() == Some(p.stable_key()) {
        s.basis = p.0.capture.frontier.reset(p.session(), s.epoch);
        s.applied = s.epoch;
        installed = true;
    } else if s.epoch == p.0.capture.epoch
        && s.applied == p.0.capture.applied
        && s.last_output == p.0.capture.previous_output
    {
        s.basis = p.0.capture.frontier.successor()?;
        installed = true;
    }
    if installed {
        s.last_output = Some(p.stable_key().into());
    }
    if s.active.as_deref() == Some(p.stable_key()) {
        s.active = None;
    }
    save_state(tx, &s)?;
    Ok(OutputCommit {
        newly_committed: true,
    })
}
pub(crate) fn terminal(tx: &mut Tx<'_>, p: &PendingOutput) -> Result<()> {
    let mut o = output(tx.conn(), p.stable_key())?;
    if o.receipt != p.0 {
        return Err(invalid("terminal receipt mismatch"));
    }
    if o.status == OutputStatus::Committed {
        return Err(invalid("known success cannot become failed"));
    }
    o.status = OutputStatus::UnresolvedTerminal;
    put(
        tx.conn(),
        &format!("{OUTPUT}{}", p.stable_key()),
        "output",
        &o,
    )?;
    let mut s = state(tx.conn(), p.session())?;
    if s.active.as_deref() == Some(p.stable_key()) {
        s.active = None;
    }
    if s.provisional
        .as_ref()
        .is_some_and(|v| v.output == p.stable_key())
    {
        s.provisional = None;
    }
    save_state(tx, &s)
}
pub(crate) fn reset_pending(tx: &mut Tx<'_>) -> Result<()> {
    for key in keys(tx.conn(), OUTPUT)? {
        let id = &key[OUTPUT.len()..];
        let o = output(tx.conn(), id)?;
        if o.status == OutputStatus::Pending {
            terminal(tx, &PendingOutput(o.receipt))?;
        }
    }
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainHold {
    pub chain_id: String,
    pub from: String,
    pub target: String,
    pub hop: u32,
    pub reason: String,
    pub at_ms: i64,
}
fn hold_key(h: &ChainHold) -> String {
    format!(
        "{HOLD}{}.{}",
        h.chain_id,
        format!("{}\0{}", h.from, h.target)
            .bytes()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}
fn decoded_chain_id(id: &str) -> Result<ChainKey> {
    let hex = id
        .strip_prefix("c1:")
        .ok_or_else(|| invalid("bad chain id prefix"))?;
    if hex.is_empty() || hex.len() % 2 != 0 || !hex.is_ascii() {
        return Err(invalid("bad chain id encoding"));
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|n| u8::from_str_radix(&hex[n..n + 2], 16).map_err(|_| invalid("bad chain id hex")))
        .collect::<Result<Vec<_>>>()?;
    let key: ChainKey =
        serde_json::from_slice(&bytes).map_err(|_| invalid("bad chain id payload"))?;
    key.validate()?;
    if key.stable_key() != id {
        return Err(invalid("noncanonical chain id"));
    }
    Ok(key)
}
fn validate_chain_id(id: &str) -> Result<()> {
    decoded_chain_id(id).map(|_| ())
}
fn validate_hold(h: &ChainHold) -> Result<()> {
    nonempty(&h.chain_id)?;
    nonempty(&h.from)?;
    nonempty(&h.target)?;
    nonempty(&h.reason)?;
    if h.at_ms < 0 {
        return Err(invalid("negative hold time"));
    }
    validate_chain_id(&h.chain_id)?;
    Ok(())
}
pub fn holds(c: &Connection, target: Option<&str>) -> Result<Vec<ChainHold>> {
    let mut v = Vec::new();
    for key in keys(c, HOLD)? {
        let h = read::<ChainHold>(c, &key, "hold")?.ok_or_else(|| invalid("hold disappeared"))?;
        validate_hold(&h)?;
        validate_event_root(c, &decoded_chain_id(&h.chain_id)?, None)?;
        if hold_key(&h) != key {
            return Err(invalid("hold key mismatch"));
        }
        if target.is_none_or(|t| h.target == t) {
            v.push(h)
        }
    }
    Ok(v)
}
pub fn hold(tx: &mut Tx<'_>, h: &ChainHold) -> Result<bool> {
    validate_hold(h)?;
    let key = hold_key(h);
    if let Some(old) = read::<ChainHold>(tx.conn(), &key, "hold")? {
        validate_hold(&old)?;
        if old.chain_id != h.chain_id || old.from != h.from || old.target != h.target {
            return Err(invalid("hold identity mismatch"));
        }
        return Ok(false);
    }
    insert(tx.conn(), &key, "hold", h)?;
    Ok(true)
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HumanRecoveryAction(String);
impl HumanRecoveryAction {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
    pub fn stable_key(&self) -> &str {
        &self.0
    }
}
impl Default for HumanRecoveryAction {
    fn default() -> Self {
        Self::new()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recovery {
    order: u64,
    action: String,
    selected: ChainHold,
    messages: Vec<Tuple>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryOrder {
    value: u64,
}

fn recovery_order(c: &Connection) -> Result<Option<u64>> {
    let order = read::<RecoveryOrder>(c, RECOVERY_ORDER, "recovery_order")?;
    if order.as_ref().is_some_and(|n| n.value == 0) {
        return Err(invalid("zero durable recovery order"));
    }
    Ok(order.map(|n| n.value))
}
fn recovery_records(c: &Connection) -> Result<Vec<Recovery>> {
    let upper = recovery_order(c)?;
    let mut out = Vec::new();
    let mut orders = BTreeSet::new();
    for key in keys(c, RECOVERY)? {
        let r =
            read::<Recovery>(c, &key, "recovery")?.ok_or_else(|| invalid("recovery missing"))?;
        uuid(&r.action)?;
        validate_hold(&r.selected)?;
        validate_event_root(c, &decoded_chain_id(&r.selected.chain_id)?, None)?;
        if key != format!("{RECOVERY}{}", r.action) {
            return Err(invalid("recovery key mismatch"));
        }
        if r.order == 0 || upper.is_none_or(|n| r.order > n) || !orders.insert(r.order) {
            return Err(invalid("invalid/duplicate durable recovery order"));
        }
        let mut seen = BTreeSet::new();
        for m in &r.messages {
            m.validate()?;
            if !seen.insert(&m.id) {
                return Err(invalid("duplicate recovery message"));
            }
        }
        out.push(r)
    }
    // UUID ordering and wall-clock resolution do not define action chronology.
    out.sort_by_key(|r| r.order);
    Ok(out)
}
fn recovered(c: &Connection, session: &str, m: &Message, f: Frontier) -> Result<Frontier> {
    let mut out = f;
    for r in recovery_records(c)? {
        if r.selected.target == session && r.messages.iter().any(|t| *t == Tuple::from(m)) {
            for v in &mut out.0 {
                if v.chain_id() == r.selected.chain_id {
                    v.key.reset = ResetIdentity::Recovery {
                        action: r.action.clone(),
                    };
                    v.hop = 0;
                }
            }
            out = Frontier::merge([out]);
        }
    }
    Ok(out)
}
fn recover_unresolved(
    c: &Connection,
    session: &str,
    m: &Message,
    f: Frontier,
) -> Result<(Frontier, bool)> {
    let mut out = f;
    let mut diagnostic_found = false;
    let mut diagnostic_budget: Option<ChainKey> = None;
    for r in recovery_records(c)? {
        if r.selected.target != session || !r.messages.iter().any(|t| *t == Tuple::from(m)) {
            continue;
        }
        if r.selected.chain_id == diagnostic_key(m).stable_key() {
            // This exact human action qualifies only the unknown part of this actual tuple.
            // Its known components retain their independent identity, hop and exact holds.
            let key = ChainKey {
                root: Root::Message {
                    id: m.id.clone(),
                    seq: m.seq,
                },
                reset: ResetIdentity::Recovery {
                    action: r.action.clone(),
                },
            };
            if let Some(previous) = diagnostic_budget.take() {
                out.0.retain(|v| v.key != previous);
            }
            out = Frontier::merge([
                out,
                Frontier(vec![Component {
                    key: key.clone(),
                    hop: 0,
                }]),
            ]);
            diagnostic_budget = Some(key);
            diagnostic_found = true;
        } else {
            for v in &mut out.0 {
                if v.chain_id() == r.selected.chain_id {
                    let is_diagnostic_budget = diagnostic_budget.as_ref() == Some(&v.key);
                    v.key.reset = ResetIdentity::Recovery {
                        action: r.action.clone(),
                    };
                    v.hop = 0;
                    if is_diagnostic_budget {
                        diagnostic_budget = Some(v.key.clone());
                    }
                }
            }
            out = Frontier::merge([out]);
            // Recovery of a known component cannot also qualify the unknown parent.
        }
    }
    Ok((out, diagnostic_found))
}
pub fn recover(
    tx: &mut Tx<'_>,
    chain_id: &str,
    from: &str,
    target: &str,
    action: &HumanRecoveryAction,
) -> Result<bool> {
    uuid(&action.0)?;
    nonempty(from)?;
    nonempty(target)?;
    validate_chain_id(chain_id)?;
    let key = format!("{RECOVERY}{}", action.0);
    let records = recovery_records(tx.conn())?;
    if let Some(r) = records.into_iter().find(|r| r.action == action.0) {
        if r.action != action.0
            || r.selected.chain_id != chain_id
            || r.selected.from != from
            || r.selected.target != target
        {
            return Err(invalid("recovery action reused for another hold"));
        }
        return Ok(false);
    }
    let Some(h) = holds(tx.conn(), Some(target))?
        .into_iter()
        .find(|h| h.chain_id == chain_id && h.from == from)
    else {
        return Ok(false);
    };
    let mut retained = Vec::new();
    for ch in crate::cursors::injection_channels(tx.conn(), target, tx.now_ms())? {
        if let Some(base) = crate::cursors::cursor(tx.conn(), target, &ch)? {
            let (ms, _) = crate::cursors::scan_unread(tx.conn(), target, &ch, base)?;
            for m in ms {
                if trusted_source(tx.conn(), &m)?.as_deref() == Some(from) {
                    let f = match read_message(tx.conn(), &m)? {
                        MessageCause::Resolved(f) => recovered(tx.conn(), target, &m, f)?,
                        MessageCause::Unresolved { previous } => {
                            recover_unresolved(tx.conn(), target, &m, previous)?.0
                        }
                    };
                    let (_, unresolved) = effective_message(tx.conn(), target, &m)?;
                    if f.0.iter().any(|v| v.chain_id() == chain_id)
                        || (unresolved && diagnostic_key(&m).stable_key() == chain_id)
                    {
                        retained.push(Tuple::from(&m));
                    }
                }
            }
        }
    }
    let order = next(recovery_order(tx.conn())?.unwrap_or(0))?;
    // Allocation, retained overlay and exact hold removal commit or roll back together.
    put(
        tx.conn(),
        RECOVERY_ORDER,
        "recovery_order",
        &RecoveryOrder { value: order },
    )?;
    insert(
        tx.conn(),
        &key,
        "recovery",
        &Recovery {
            order,
            action: action.0.clone(),
            selected: h.clone(),
            messages: retained,
        },
    )?;
    tx.conn()
        .execute("DELETE FROM meta WHERE key=?1", [hold_key(&h)])?;
    Ok(true)
}
pub(crate) fn guard_crew(c: &Connection, crew: &str) -> Result<()> {
    for key in keys(c, OUTPUT)? {
        let o = output(c, &key[OUTPUT.len()..])?;
        if o.status == OutputStatus::Pending {
            let reader = crewd_store::sessions::get(c, &o.receipt.session)?;
            if reader.as_ref().is_some_and(|s| s.crew_id == crew) {
                return Err(invalid("crew has unresolved causal owner"));
            }
        }
    }
    Ok(())
}
fn collect_chain_output(key: &ChainKey, outputs: &mut BTreeSet<String>) {
    if let Root::Event { output, .. } = &key.root {
        outputs.insert(output.clone());
    }
}
fn collect_event_outputs(frontier: &Frontier, outputs: &mut BTreeSet<String>) -> Result<()> {
    frontier.validate()?;
    for component in &frontier.0 {
        collect_chain_output(&component.key, outputs);
    }
    Ok(())
}
pub(crate) fn prune_after_cascade(tx: &mut Tx<'_>) -> Result<()> {
    for key in keys(tx.conn(), MESSAGE)? {
        let id = &key[MESSAGE.len()..];
        if let Some(m) = crewd_store::messages::get(tx.conn(), id)? {
            read_message(tx.conn(), &m)?;
        } else {
            tx.conn().execute("DELETE FROM meta WHERE key=?1", [key])?;
        }
    }
    for key in keys(tx.conn(), SESSION)? {
        let id = &key[SESSION.len()..];
        state(tx.conn(), id)?;
        if crewd_store::sessions::get(tx.conn(), id)?.is_none() {
            tx.conn().execute("DELETE FROM meta WHERE key=?1", [key])?;
        }
    }
    // Validate every present output before deleting any: malformed terminal records are not
    // silently treated as absent, even when no surviving ancestry happens to reference them.
    let mut records = BTreeMap::new();
    for key in keys(tx.conn(), OUTPUT)? {
        let id = key[OUTPUT.len()..].to_string();
        records.insert(id.clone(), output(tx.conn(), &id)?);
    }
    let mut reachable = BTreeSet::new();
    for key in keys(tx.conn(), MESSAGE)? {
        let stamp = read::<Stamp>(tx.conn(), &key, "message")?
            .ok_or_else(|| invalid("surviving causal stamp disappeared"))?;
        if let Some(id) = &stamp.candidate {
            reachable.insert(id.clone());
        }
        match &stamp.resolution {
            Resolution::Resolved { frontier } => collect_event_outputs(frontier, &mut reachable)?,
            Resolution::Eligible {
                output,
                frontier,
                success,
            } => {
                reachable.insert(output.clone());
                reachable.insert(success.receipt.id.clone());
                collect_event_outputs(frontier, &mut reachable)?;
            }
            Resolution::Unresolved {
                previous,
                candidate,
            } => {
                reachable.insert(candidate.clone());
                collect_event_outputs(previous, &mut reachable)?;
            }
        }
    }
    for key in keys(tx.conn(), SESSION)? {
        let session = state(tx.conn(), &key[SESSION.len()..])?;
        collect_event_outputs(&session.basis, &mut reachable)?;
        if let Some(id) = session.last_output {
            reachable.insert(id);
        }
        if let Some(id) = session.active {
            reachable.insert(id);
        }
        if let Some(provisional) = session.provisional {
            reachable.insert(provisional.output);
            collect_event_outputs(&provisional.basis, &mut reachable)?;
        }
    }
    // Human episodes and once-action overlays remain roots, including foreign history after
    // a source disappears. Purge does not automatically clear holds or reset action identity.
    for h in holds(tx.conn(), None)? {
        collect_chain_output(&decoded_chain_id(&h.chain_id)?, &mut reachable);
    }
    for recovery in recovery_records(tx.conn())? {
        collect_chain_output(
            &decoded_chain_id(&recovery.selected.chain_id)?,
            &mut reachable,
        );
    }
    for (id, record) in &records {
        if record.status == OutputStatus::Pending {
            reachable.insert(id.clone());
        }
    }
    // Follow the exact durable captured components, not current session/row existence or
    // source-name prefixes. A visited set bounds even corrupted cross-reference cycles.
    let mut visited = BTreeSet::new();
    loop {
        let next = reachable.iter().find(|id| !visited.contains(*id)).cloned();
        let Some(id) = next else { break };
        let record = records
            .get(&id)
            .ok_or_else(|| invalid("referenced original causal output missing"))?;
        visited.insert(id);
        validate_event_frontier(tx.conn(), &record.receipt.capture.frontier, None)?;
        collect_event_outputs(&record.receipt.capture.frontier, &mut reachable)?;
        for part in &record.receipt.capture.parts {
            validate_event_frontier(tx.conn(), part, None)?;
            collect_event_outputs(part, &mut reachable)?;
        }
    }
    for id in records.keys().filter(|id| !reachable.contains(*id)) {
        tx.conn()
            .execute("DELETE FROM meta WHERE key=?1", [format!("{OUTPUT}{id}")])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    include!("causal/tests.rs");
    include!("causal/admission_tests.rs");
}
