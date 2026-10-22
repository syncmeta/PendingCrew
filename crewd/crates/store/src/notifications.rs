//! Optional notification facts; only caller-owned FIFO transactions mutate these.
//! Business state remains authoritative. No external send occurs in this crate.
use crate::{Result, StoreError};
use crewd_proto::notifications::*;
use ring::digest::{SHA256, digest};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub const DDL: &str = r#"
CREATE TABLE notification_settings(singleton INTEGER PRIMARY KEY CHECK(singleton=1),revision INTEGER NOT NULL CHECK(revision>0),enabled INTEGER NOT NULL CHECK(enabled IN(0,1)),binding TEXT CHECK(binding IS NULL OR json_valid(binding)));
CREATE TABLE notification_installations(id TEXT PRIMARY KEY,authority_id TEXT NOT NULL,db_id TEXT NOT NULL,grant_id TEXT NOT NULL,grant_revision TEXT NOT NULL,client_pin TEXT NOT NULL,generation INTEGER NOT NULL CHECK(generation>0),revoked INTEGER NOT NULL CHECK(revoked IN(0,1)),summary TEXT NOT NULL CHECK(json_valid(summary)),token BLOB);
CREATE INDEX notification_installation_grant ON notification_installations(grant_id);
CREATE TABLE notification_requests(request_id TEXT PRIMARY KEY,owner TEXT NOT NULL,operation TEXT NOT NULL,digest TEXT NOT NULL,result TEXT CHECK(result IS NULL OR json_valid(result)));
CREATE TABLE notification_outbox(id INTEGER PRIMARY KEY AUTOINCREMENT,installation_id TEXT NOT NULL REFERENCES notification_installations(id),source_key TEXT NOT NULL,context TEXT NOT NULL CHECK(json_valid(context)),session_id TEXT,generation TEXT,enqueued_at INTEGER NOT NULL,expires_at INTEGER NOT NULL,state TEXT NOT NULL CHECK(state IN('pending','unknown','accepted','rejected','suppressed')),attempt TEXT UNIQUE,attempt_generation INTEGER,reason TEXT,UNIQUE(installation_id,source_key));
CREATE INDEX notification_pending ON notification_outbox(state,id);
"#;
fn invalid(s: &str) -> StoreError {
    StoreError::Invalid(s.into())
}
fn tx(c: &Connection) -> Result<()> {
    if c.is_autocommit() {
        Err(invalid("notification mutation requires owner transaction"))
    } else {
        Ok(())
    }
}
pub fn hash(data: &[u8]) -> String {
    digest(&SHA256, data)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn json<T: Serialize>(v: &T) -> Result<String> {
    Ok(serde_json::to_string(v)?)
}
pub fn number(s: &str, zero: bool) -> Result<i64> {
    let n = s
        .parse::<i64>()
        .map_err(|_| invalid("invalid notification revision"))?;
    if n.to_string() != s || n < if zero { 0 } else { 1 } {
        return Err(invalid("invalid notification revision"));
    }
    Ok(n)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderBinding {
    pub authority_id: String,
    pub db_id: String,
    pub team_id: String,
    pub key_id: String,
    pub environment: PushEnvironment,
    pub topic: String,
    pub key_slot: String,
    pub seal_slot: String,
}
#[derive(Clone, Debug)]
pub struct Settings {
    pub revision: i64,
    pub enabled: bool,
    pub binding: Option<ProviderBinding>,
}
pub fn settings(c: &Connection) -> Result<Settings> {
    let row: Option<(i64, bool, Option<String>)> = c
        .query_row(
            "SELECT revision,enabled,binding FROM notification_settings WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    match row {
        Some((revision, enabled, binding)) => Ok(Settings {
            revision,
            enabled,
            binding: binding.map(|s| serde_json::from_str(&s)).transpose()?,
        }),
        None => Ok(Settings {
            revision: 0,
            enabled: false,
            binding: None,
        }),
    }
}
#[derive(Clone, Debug)]
pub struct Owner {
    pub authority_id: String,
    pub db_id: String,
    pub grant_id: String,
    pub grant_revision: String,
    pub client_pin: String,
}
impl Owner {
    pub fn key(&self) -> String {
        format!("{}:{}:{}", self.authority_id, self.db_id, self.grant_id)
    }
    pub fn authorize(&self, c: &Connection) -> Result<()> {
        let identity = crate::remote::identity(c)?
            .ok_or_else(|| invalid("notification authority unavailable"))?;
        if identity.authority_id != self.authority_id
            || identity.db_id != self.db_id
            || crate::meta_get(c, "dbId")?.as_deref() != Some(self.db_id.as_str())
        {
            return Err(invalid("notification identity mismatch"));
        }
        let grant = crate::remote::grant(c, &self.grant_id, &self.authority_id)?
            .ok_or_else(|| invalid("notification owner unavailable"))?;
        if grant.revoked_at.is_some()
            || grant.revision != self.grant_revision
            || grant.client_spki_sha256 != self.client_pin
            || !crate::remote::configuration(c)?.enabled
        {
            return Err(invalid("notification owner revoked"));
        }
        Ok(())
    }
}
pub fn request_lookup(
    c: &Connection,
    owner: &str,
    request: &str,
) -> Result<NotificationRequestLookup> {
    let row: Option<(String, String, Option<String>)> = c
        .query_row(
            "SELECT owner,operation,result FROM notification_requests WHERE request_id=?1",
            [request],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    match row {
        None => Ok(NotificationRequestLookup::NotFound),
        Some((o, _, _)) if o != owner => Err(invalid("notification request owner mismatch")),
        Some((_, operation, None)) => Ok(NotificationRequestLookup::Unknown { operation }),
        Some((_, operation, Some(result))) => Ok(NotificationRequestLookup::Known {
            operation,
            result: serde_json::from_str(&result)?,
        }),
    }
}
pub fn existing_request(
    c: &Connection,
    owner: &str,
    request: &str,
    operation: &str,
    body_hash: &str,
) -> Result<Option<NotificationRequestLookup>> {
    let old: Option<(String, String, String)> = c
        .query_row(
            "SELECT owner,operation,digest FROM notification_requests WHERE request_id=?1",
            [request],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    match old {
        None => Ok(None),
        Some(old) if old != (owner.into(), operation.into(), body_hash.into()) => {
            Err(invalid("notification original request conflict"))
        }
        Some(_) => Ok(Some(request_lookup(c, owner, request)?)),
    }
}
/// True means new. Existing identical UUID is read-only, never redoes an effect.
pub fn reserve(
    c: &Connection,
    owner: &str,
    request: &str,
    operation: &str,
    body_hash: &str,
) -> Result<bool> {
    tx(c)?;
    if !crewd_proto::valid_remote_uuid(request) || body_hash.len() != 64 {
        return Err(invalid("invalid notification request"));
    }
    let old: Option<(String, String, String)> = c
        .query_row(
            "SELECT owner,operation,digest FROM notification_requests WHERE request_id=?1",
            [request],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some(old) = old {
        if old != (owner.into(), operation.into(), body_hash.into()) {
            return Err(invalid("notification original request conflict"));
        }
        return Ok(false);
    }
    c.execute(
        "INSERT INTO notification_requests(request_id,owner,operation,digest) VALUES(?1,?2,?3,?4)",
        params![request, owner, operation, body_hash],
    )?;
    Ok(true)
}
pub fn result(c: &Connection, owner: &str, request: &str, value: &serde_json::Value) -> Result<()> {
    tx(c)?;
    if c.execute("UPDATE notification_requests SET result=?1 WHERE request_id=?2 AND owner=?3 AND result IS NULL",params![json(value)?,request,owner])?!=1{return Err(invalid("notification result no longer pending"))}
    Ok(())
}
pub fn configure(
    c: &Connection,
    expected: i64,
    enabled: bool,
    binding: Option<&ProviderBinding>,
) -> Result<Settings> {
    tx(c)?;
    let old = settings(c)?;
    if old.revision != expected || expected == i64::MAX || enabled && binding.is_none() {
        return Err(invalid("notification settings CAS conflict"));
    }
    let rev = expected + 1;
    c.execute("INSERT INTO notification_settings(singleton,revision,enabled,binding) VALUES(1,?1,?2,?3) ON CONFLICT(singleton) DO UPDATE SET revision=excluded.revision,enabled=excluded.enabled,binding=excluded.binding",params![rev,enabled,binding.map(json).transpose()?])?;
    // A provider change never sends old pending observations through a new binding.
    c.execute("UPDATE notification_outbox SET state='suppressed',reason='configuration_changed' WHERE state='pending'",[])?;
    settings(c)
}
pub fn installation(
    c: &Connection,
    owner: &Owner,
    id: &str,
) -> Result<Option<NotificationInstallation>> {
    let row: Option<(String, String, String)> = c
        .query_row(
            "SELECT grant_id,client_pin,summary FROM notification_installations WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    match row {
        None => Ok(None),
        Some((grant, pin, _)) if grant != owner.grant_id || pin != owner.client_pin => {
            Err(invalid("notification installation owner mismatch"))
        }
        Some((_, _, summary)) => Ok(Some(serde_json::from_str(&summary)?)),
    }
}
pub fn token_aad(owner: &Owner, summary: &NotificationInstallation) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&(
        owner.key(),
        &summary.installation_id,
        &summary.generation,
        summary.environment,
        &summary.topic,
    ))?)
}
pub fn register(
    c: &Connection,
    owner: &Owner,
    p: &NotificationRegisterParams,
    sealed: &[u8],
) -> Result<NotificationInstallation> {
    tx(c)?;
    owner.authorize(c)?;
    if !crewd_proto::valid_remote_uuid(&p.installation_id)
        || p.crew_ids.is_empty()
        || p.crew_ids.len() > 64
        || p.kinds.is_empty()
        || p.kinds.len() > 8
        || sealed.len() < 28
        || sealed.len() > 1024
    {
        return Err(invalid("invalid notification installation"));
    }
    let config = settings(c)?;
    let binding = config
        .binding
        .ok_or_else(|| invalid("notifications not configured"))?;
    if !config.enabled
        || binding.authority_id != owner.authority_id
        || binding.db_id != owner.db_id
        || p.topic != binding.topic
        || p.environment != binding.environment
    {
        return Err(invalid("notification provider scope mismatch"));
    }
    for (i, crew) in p.crew_ids.iter().enumerate() {
        if p.crew_ids[..i].contains(crew) || !crew_live(c, crew)? {
            return Err(invalid("notification crew subscription denied"));
        }
    }
    for (i, kind) in p.kinds.iter().enumerate() {
        if p.kinds[..i].contains(kind) {
            return Err(invalid("duplicate notification kind"));
        }
    }
    let expected = number(&p.expected_generation, true)?;
    let old = installation(c, owner, &p.installation_id)?;
    if old.as_ref().is_some_and(|v| v.revoked)
        || old
            .as_ref()
            .map(|v| number(&v.generation, false))
            .transpose()?
            .unwrap_or(0)
            != expected
        || expected == i64::MAX
    {
        return Err(invalid("notification installation CAS conflict"));
    }
    if old.is_none() {
        let count: i64 =
            c.query_row("SELECT COUNT(*) FROM notification_installations", [], |r| {
                r.get(0)
            })?;
        if count >= 256 {
            return Err(invalid("notification installation bound reached"));
        }
    }
    let value = NotificationInstallation {
        installation_id: p.installation_id.clone(),
        generation: (expected + 1).to_string(),
        revoked: false,
        environment: p.environment,
        topic: p.topic.clone(),
        crew_ids: p.crew_ids.clone(),
        kinds: p.kinds.clone(),
    };
    c.execute("INSERT INTO notification_installations(id,authority_id,db_id,grant_id,grant_revision,client_pin,generation,revoked,summary,token) VALUES(?1,?2,?3,?4,?5,?6,?7,0,?8,?9) ON CONFLICT(id) DO UPDATE SET generation=excluded.generation,summary=excluded.summary,token=excluded.token",params![p.installation_id,owner.authority_id,owner.db_id,owner.grant_id,owner.grant_revision,owner.client_pin,expected+1,json(&value)?,sealed])?;
    Ok(value)
}
pub fn revoke(
    c: &Connection,
    owner: &Owner,
    id: &str,
    expected: i64,
) -> Result<NotificationInstallation> {
    tx(c)?;
    owner.authorize(c)?;
    let mut value =
        installation(c, owner, id)?.ok_or_else(|| invalid("notification installation missing"))?;
    if value.revoked || number(&value.generation, false)? != expected || expected == i64::MAX {
        return Err(invalid("notification installation CAS conflict"));
    }
    value.generation = (expected + 1).to_string();
    value.revoked = true;
    c.execute("UPDATE notification_installations SET generation=?1,revoked=1,summary=?2,token=NULL WHERE id=?3",params![expected+1,json(&value)?,id])?;
    c.execute("UPDATE notification_outbox SET state='suppressed',reason='installation_revoked' WHERE installation_id=?1 AND state='pending'",[id])?;
    Ok(value)
}
fn crew_live(c: &Connection, crew: &str) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM crews WHERE id=?1 AND archived_at IS NULL)",
        [crew],
        |r| r.get(0),
    )?)
}
fn matching_installations(
    c: &Connection,
    binding: &ProviderBinding,
    crew: &str,
    kind: NotificationKind,
) -> Result<Vec<String>> {
    let mut query=c.prepare("SELECT id,grant_id,grant_revision,client_pin,summary FROM notification_installations WHERE revoked=0 AND authority_id=?1 AND db_id=?2 ORDER BY id LIMIT 257")?;
    let rows = query
        .query_map(params![binding.authority_id, binding.db_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.len() > 256 {
        return Err(invalid("notification installation bound exceeded"));
    }
    let mut matched = Vec::new();
    for (id, grant_id, grant_revision, client_pin, summary) in rows {
        let summary: NotificationInstallation = serde_json::from_str(&summary)?;
        let owner = Owner {
            authority_id: binding.authority_id.clone(),
            db_id: binding.db_id.clone(),
            grant_id,
            grant_revision,
            client_pin,
        };
        if summary.topic != binding.topic
            || summary.environment != binding.environment
            || owner.authorize(c).is_err()
            || !summary.crew_ids.iter().any(|v| v == crew)
            || !summary.kinds.contains(&kind)
        {
            continue;
        }
        matched.push(id);
    }
    Ok(matched)
}
/// Only called after a genuine source event is admitted by its owning producer.
/// Zero configured/matching subscriptions creates no observation rows.
pub fn enqueue(
    c: &Connection,
    crew: &str,
    kind: NotificationKind,
    source: NotificationSource,
    session: Option<(&str, &str)>,
    now: i64,
) -> Result<()> {
    tx(c)?;
    let config = settings(c)?;
    if !config.enabled {
        return Ok(());
    }
    let Some(binding) = config.binding else {
        return Ok(());
    };
    if !crew_live(c, crew)? || now < 0 {
        return Ok(());
    }
    let valid_kind = matches!(
        (&source, kind),
        (
            NotificationSource::WorkEvent { .. },
            NotificationKind::Done
                | NotificationKind::Reported
                | NotificationKind::Blocked
                | NotificationKind::AskOpened
        ) | (
            NotificationSource::CodexTurn { .. } | NotificationSource::ManagedResponse { .. },
            NotificationKind::ReplyReady | NotificationKind::ResponseFailed
        ) | (
            NotificationSource::Decision { .. },
            NotificationKind::HumanApprovalRequired
        ) | (
            NotificationSource::Message { .. },
            NotificationKind::HumanMention
        )
    );
    if !valid_kind {
        return Err(invalid("notification source/kind mismatch"));
    }
    let context = NotificationContext {
        action: "open_context".into(),
        authority_id: binding.authority_id.clone(),
        db_id: binding.db_id.clone(),
        crew_id: crew.into(),
        kind,
        source,
    };
    let raw = json(&context)?;
    if raw.len() > 3072 {
        return Err(invalid("notification source context exceeds bound"));
    }
    // Dedupe uses original source identity, never state labels, token generation,
    // receipt revision, public display metadata, or a new delivery UUID.
    let original = match &context.source {
        NotificationSource::WorkEvent { event_seq, .. } => json(&("work", event_seq))?,
        NotificationSource::CodexTurn {
            session_id,
            thread_id,
            turn_id,
        } => json(&("codexTurn", session_id, thread_id, turn_id))?,
        NotificationSource::ManagedResponse {
            session_id,
            runtime_id,
            request_id,
            prompt_id,
            ..
        } => json(&(
            "managedResponse",
            session_id,
            runtime_id,
            request_id,
            prompt_id,
        ))?,
        NotificationSource::Decision {
            session_id,
            generation,
            decision_id,
        } => json(&("decision", session_id, generation, decision_id))?,
        NotificationSource::Message {
            message_id,
            message_seq,
            channel_id,
        } => json(&("message", message_id, message_seq, channel_id))?,
    };
    let source_key =
        hash(json(&(&binding.authority_id, &binding.db_id, crew, original))?.as_bytes());
    for id in matching_installations(c, &binding, crew, kind)? {
        let expires = now
            .checked_add(86_400_000)
            .ok_or_else(|| invalid("notification expiry overflow"))?;
        c.execute("INSERT INTO notification_outbox(installation_id,source_key,context,session_id,generation,enqueued_at,expires_at,state) VALUES(?1,?2,?3,?4,?5,?6,?7,'pending') ON CONFLICT(installation_id,source_key) DO NOTHING",params![id,source_key,raw,session.map(|v|v.0),session.map(|v|v.1),now,expires])?;
    }
    Ok(())
}
pub fn enqueue_work(c: &Connection, crew: &str, event: &crewd_work::Event) -> Result<()> {
    // Historical pre-activation migration seams have no notification writer.
    // Production Store admission enforces current19 before any public mutation.
    let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 19 {
        return Ok(());
    }
    use crate::object_refs::ObjectKind;
    use crewd_work::EventKind::*;
    if let AskAnswered { ask, .. } | AskDefaulted { ask, .. } | AskWithdrawn { ask, .. } =
        &event.kind
    {
        c.execute("UPDATE notification_outbox SET state='suppressed',reason='ask_resolved' WHERE state='pending' AND json_extract(context,'$.crewId')=?1 AND json_extract(context,'$.source.type')='workEvent' AND json_extract(context,'$.source.askId')=?2",params![crew,ask.to_string()])?;
        return Ok(());
    }
    let (kind, work, ask, object) = match &event.kind {
        Done { id, .. } => (NotificationKind::Done, Some(*id), None, ObjectKind::Plan),
        Reported { id, .. } => (
            NotificationKind::Reported,
            Some(*id),
            None,
            ObjectKind::Plan,
        ),
        Blocked { id, .. } => (NotificationKind::Blocked, Some(*id), None, ObjectKind::Plan),
        AskOpened { ask, item, .. } => (
            NotificationKind::AskOpened,
            *item,
            Some(*ask),
            ObjectKind::HumanTask,
        ),
        _ => return Ok(()),
    };
    // Do not resolve/index anything while notification delivery is unconfigured.
    let config = settings(c)?;
    if !config.enabled {
        return Ok(());
    }
    let Some(binding) = config.binding else {
        return Ok(());
    };
    if matching_installations(c, &binding, crew, kind)?.is_empty() {
        return Ok(());
    }
    let core = ask
        .map(|v| v.to_string())
        .or_else(|| work.map(|v| v.to_string()))
        .ok_or_else(|| invalid("notification source ID missing"))?;
    let reference = crate::work_organization::reference_for_core(c, object, &core)?
        .ok_or_else(|| invalid("notification public source identity missing"))?;
    enqueue(
        c,
        crew,
        kind,
        NotificationSource::WorkEvent {
            event_seq: event.seq.to_string(),
            work_id: work.map(|v| v.to_string()),
            ask_id: ask.map(|v| v.to_string()),
            entry_no: None,
            object_id: reference.id(),
        },
        None,
        event.at.0,
    )
}
/// These reminders represent a current live observation, not permanent work truth.
pub fn grant_revoked(c: &Connection, grant_id: &str) -> Result<()> {
    tx(c)?;
    c.execute("UPDATE notification_outbox SET state='suppressed',reason='grant_revoked' WHERE state='pending' AND installation_id IN(SELECT id FROM notification_installations WHERE grant_id=?1)",[grant_id])?;
    let mut query = c.prepare(
        "SELECT id,summary FROM notification_installations WHERE grant_id=?1 AND revoked=0",
    )?;
    let rows = query
        .query_map([grant_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(query);
    for (id, raw) in rows {
        let mut summary: NotificationInstallation = serde_json::from_str(&raw)?;
        summary.revoked = true;
        c.execute(
            "UPDATE notification_installations SET revoked=1,token=NULL,summary=?1 WHERE id=?2",
            params![json(&summary)?, id],
        )?;
    }
    Ok(())
}

pub fn suppress_session(c: &Connection, session: &str, reason: &str) -> Result<()> {
    tx(c)?;
    if c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? < 19 {
        return Ok(());
    }
    c.execute("UPDATE notification_outbox SET state='suppressed',reason=?1 WHERE session_id=?2 AND state='pending'",params![reason,session])?;
    Ok(())
}
pub fn restart(c: &Connection) -> Result<()> {
    tx(c)?;
    c.execute("UPDATE notification_outbox SET state='suppressed',reason='runtime_reopened' WHERE session_id IS NOT NULL AND state='pending'",[])?;
    Ok(())
}
pub fn restored(c: &Connection) -> Result<()> {
    tx(c)?;
    let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 19 {
        return Ok(());
    }
    c.execute("UPDATE notification_settings SET enabled=0", [])?;
    let mut q = c.prepare("SELECT id,summary FROM notification_installations WHERE revoked=0")?;
    let rows = q
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(q);
    for (id, raw) in rows {
        let mut value: NotificationInstallation = serde_json::from_str(&raw)?;
        value.revoked = true;
        c.execute(
            "UPDATE notification_installations SET revoked=1,token=NULL,summary=?1 WHERE id=?2",
            params![json(&value)?, id],
        )?;
    }
    c.execute("UPDATE notification_outbox SET state='suppressed',reason='database_restored' WHERE state='pending'",[])?;
    Ok(())
}
pub struct Claimed {
    pub delivery_id: i64,
    pub session_generation: Option<String>,
    pub installation: NotificationInstallation,
    pub owner: Owner,
    pub sealed_token: Vec<u8>,
    pub context: NotificationContext,
    pub attempt: String,
    pub expires_at: i64,
    pub config_revision: i64,
}
/// Unknown is durable before returning the external-I/O permit. Never selects Unknown again.
pub fn claim(c: &Connection, now: i64, expected_revision: i64) -> Result<Option<Claimed>> {
    claim_inner(c, now, expected_revision, None, true)
}
pub fn claim_checked(
    c: &Connection,
    now: i64,
    expected_revision: i64,
    expected_id: i64,
) -> Result<Option<Claimed>> {
    claim_inner(c, now, expected_revision, Some(expected_id), true)
}
pub fn peek(c: &Connection, now: i64, expected_revision: i64) -> Result<Option<Claimed>> {
    claim_inner(c, now, expected_revision, None, false)
}
fn claim_inner(
    c: &Connection,
    now: i64,
    expected_revision: i64,
    expected_id: Option<i64>,
    mark_unknown: bool,
) -> Result<Option<Claimed>> {
    tx(c)?;
    let config = settings(c)?;
    if !config.enabled || config.revision != expected_revision {
        return Ok(None);
    }
    c.execute("UPDATE notification_outbox SET state='suppressed',reason='expired' WHERE state='pending' AND expires_at<=?1",[now])?;
    let candidate:Option<(i64,String,String,i64,Option<String>)>=c.query_row("SELECT id,installation_id,context,expires_at,generation FROM notification_outbox WHERE state='pending' ORDER BY id LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    let Some((delivery_id, id, context, expires_at, session_generation)) = candidate else {
        return Ok(None);
    };
    if expected_id.is_some_and(|id| id != delivery_id) {
        return Ok(None);
    }
    let context: NotificationContext = serde_json::from_str(&context)?;
    let row:(String,String,String,String,String,String,Option<Vec<u8>>,bool)=c.query_row("SELECT authority_id,db_id,grant_id,grant_revision,client_pin,summary,token,revoked FROM notification_installations WHERE id=?1",[&id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?)))?;
    let owner = Owner {
        authority_id: row.0,
        db_id: row.1,
        grant_id: row.2,
        grant_revision: row.3,
        client_pin: row.4,
    };
    let installation: NotificationInstallation = serde_json::from_str(&row.5)?;
    let live = source_live(c, &context)?
        && !row.7
        && row.6.is_some()
        && owner.authorize(c).is_ok()
        && crew_live(c, &context.crew_id)?
        && installation.crew_ids.contains(&context.crew_id)
        && installation.kinds.contains(&context.kind)
        && config.binding.as_ref().is_some_and(|b| {
            b.authority_id == context.authority_id
                && b.db_id == context.db_id
                && b.environment == installation.environment
                && b.topic == installation.topic
        });
    if !live {
        c.execute("UPDATE notification_outbox SET state='suppressed',reason='authority_changed' WHERE id=?1 AND state='pending'",[delivery_id])?;
        return Ok(None);
    }
    let attempt = if mark_unknown {
        uuid::Uuid::new_v4().to_string()
    } else {
        String::new()
    };
    let generation = number(&installation.generation, false)?;
    if mark_unknown && c.execute("UPDATE notification_outbox SET state='unknown',attempt=?1,attempt_generation=?2 WHERE id=?3 AND state='pending'",params![attempt,generation,delivery_id])?!=1{return Err(invalid("notification claim conflict"))}
    Ok(Some(Claimed {
        delivery_id,
        session_generation,
        installation,
        owner,
        sealed_token: row.6.unwrap(),
        context,
        attempt,
        expires_at,
        config_revision: config.revision,
    }))
}
fn source_live(c: &Connection, context: &NotificationContext) -> Result<bool> {
    match &context.source {
        NotificationSource::ManagedResponse{session_id,runtime_id,request_id,prompt_id,receipt_revision}=>{
            let revision=number(receipt_revision,true)?;
            let failed=context.kind==NotificationKind::ResponseFailed;
            Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM native_execution_receipts e JOIN native_execution_runtimes r ON r.runtime_id=e.runtime_id JOIN sessions s ON s.id=r.session_id WHERE e.request_id=?1 AND e.runtime_id=?2 AND e.prompt_id=?3 AND e.revision=?4 AND e.phase=?6 AND e.turn_id IS NULL AND (r.revoked=0 OR ?7) AND r.manual=0 AND r.session_id=?5 AND s.desired='running' AND s.was_working=0)",params![request_id,runtime_id,prompt_id,revision,session_id,if failed{"unknown"}else{"completed"},failed],|r|r.get(0))?)
        }
        NotificationSource::CodexTurn{session_id,..}=>Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1 AND desired='running' AND was_working=0)",[session_id],|r|r.get(0))?),
        NotificationSource::Decision{session_id,..}=>Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1 AND desired='running')",[session_id],|r|r.get(0))?),
        NotificationSource::Message{message_id,message_seq,channel_id}=>Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM messages WHERE id=?1 AND seq=?2 AND channel_id=?3)",params![message_id,number(message_seq,false)?,channel_id],|r|r.get(0))?),
        NotificationSource::WorkEvent{..}=>Ok(true),
    }
}
pub fn suppress_unclaimed(c: &Connection, id: i64, reason: &str) -> Result<()> {
    tx(c)?;
    c.execute("UPDATE notification_outbox SET state='suppressed',reason=?1 WHERE id=?2 AND state='pending'",params![reason,id])?;
    Ok(())
}
pub fn still_allowed(c: &Connection, claimed: &Claimed, now: i64) -> Result<bool> {
    let config = settings(c)?;
    Ok(source_live(c, &claimed.context)?
        && config.enabled
        && config.revision == claimed.config_revision
        && claimed.expires_at > now
        && claimed.owner.authorize(c).is_ok()
        && installation(c, &claimed.owner, &claimed.installation.installation_id)?
            .is_some_and(|s| !s.revoked && s.generation == claimed.installation.generation)
        && crew_live(c, &claimed.context.crew_id)?)
}
pub fn settle(
    c: &Connection,
    id: i64,
    attempt: &str,
    state: NotificationDeliveryState,
    reason: Option<&str>,
    invalidate_token: bool,
) -> Result<()> {
    tx(c)?;
    let target = match state {
        NotificationDeliveryState::AcceptedByApns => "accepted",
        NotificationDeliveryState::Rejected => "rejected",
        NotificationDeliveryState::Suppressed => "suppressed",
        NotificationDeliveryState::Unknown => return Ok(()),
        NotificationDeliveryState::Pending => return Err(invalid("notification cannot requeue")),
    };
    if c.execute("UPDATE notification_outbox SET state=?1,reason=?2 WHERE id=?3 AND attempt=?4 AND state='unknown'",params![target,reason,id,attempt])?!=1{return Err(invalid("notification original attempt conflict"))}
    if invalidate_token {
        let (installation_id,generation):(String,i64)=c.query_row("SELECT installation_id,attempt_generation FROM notification_outbox WHERE id=?1 AND attempt=?2",params![id,attempt],|r|Ok((r.get(0)?,r.get(1)?)))?;
        let summary:Option<String>=c.query_row("SELECT summary FROM notification_installations WHERE id=?1 AND generation=?2 AND revoked=0",params![installation_id,generation],|r|r.get(0)).optional()?;
        if let Some(raw) = summary {
            let mut value: NotificationInstallation = serde_json::from_str(&raw)?;
            value.revoked = true;
            c.execute("UPDATE notification_installations SET revoked=1,token=NULL,summary=?1 WHERE id=?2 AND generation=?3",params![json(&value)?,installation_id,generation])?;
            c.execute("UPDATE notification_outbox SET state='suppressed',reason='invalid_token' WHERE installation_id=?1 AND state='pending'",[installation_id])?;
        }
    }
    Ok(())
}
fn delivery_row(
    id: i64,
    context: String,
    state: String,
    apns_id: Option<String>,
    reason: Option<String>,
    expiry: i64,
) -> Result<NotificationDelivery> {
    let state = match state.as_str() {
        "pending" => NotificationDeliveryState::Pending,
        "unknown" => NotificationDeliveryState::Unknown,
        "accepted" => NotificationDeliveryState::AcceptedByApns,
        "rejected" => NotificationDeliveryState::Rejected,
        "suppressed" => NotificationDeliveryState::Suppressed,
        _ => return Err(invalid("invalid notification state")),
    };
    Ok(NotificationDelivery {
        delivery_id: id.to_string(),
        context: serde_json::from_str(&context)?,
        state,
        apns_id,
        reason,
        expires_at: crate::ts_from_ms(expiry),
    })
}
pub fn delivery_get(
    c: &Connection,
    owner: &Owner,
    p: &NotificationDeliveryGetParams,
) -> Result<Option<NotificationDelivery>> {
    owner.authorize(c)?;
    let id = number(&p.delivery_id, false)?;
    if !crewd_proto::valid_remote_uuid(&p.installation_id) {
        return Err(invalid("invalid notification installation"));
    }
    let Some(installation) = installation(c, owner, &p.installation_id)? else {
        return Ok(None);
    };
    if installation.revoked {
        return Ok(None);
    }
    let row=c.query_row("SELECT id,context,state,attempt,reason,expires_at FROM notification_outbox WHERE installation_id=?1 AND id=?2",params![p.installation_id,id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,i64>(5)?))).optional()?;
    row.map(|(id, context, state, apns_id, reason, expiry)| {
        delivery_row(id, context, state, apns_id, reason, expiry)
    })
    .transpose()
}
pub fn deliveries(
    c: &Connection,
    owner: &Owner,
    p: &NotificationDeliveryListParams,
) -> Result<NotificationDeliveryPage> {
    owner.authorize(c)?;
    installation(c, owner, &p.installation_id)?
        .ok_or_else(|| invalid("notification installation missing"))?;
    if p.limit == 0 || p.limit > 100 {
        return Err(invalid("notification page bound"));
    }
    let after = p
        .after_delivery_id
        .as_deref()
        .map(|v| number(v, true))
        .transpose()?
        .unwrap_or(0);
    let mut q=c.prepare("SELECT id,context,state,attempt,reason,expires_at FROM notification_outbox WHERE installation_id=?1 AND id>?2 ORDER BY id LIMIT ?3")?;
    let rows = q
        .query_map(params![p.installation_id, after, p.limit + 1], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let more = rows.len() > p.limit as usize;
    let mut items = Vec::new();
    for (id, context, state, apns_id, reason, expiry) in rows.into_iter().take(p.limit as usize) {
        items.push(delivery_row(id, context, state, apns_id, reason, expiry)?);
    }
    Ok(NotificationDeliveryPage {
        next_after_delivery_id: if more {
            items.last().map(|v| v.delivery_id.clone())
        } else {
            None
        },
        items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;
    const AUTH: &str = "01234567-89ab-4cde-8fab-0123456789ab";
    fn setup(s: &Store) -> Owner {
        let c = s.conn();
        c.execute_batch("INSERT INTO crews(id,title,created_at,updated_at) VALUES('crew','Synthetic','t','t'); UPDATE remote_configuration SET enabled=1;").unwrap();
        let db_id = crate::meta_get(c, "dbId").unwrap().unwrap();
        crate::remote::bind_identity(
            c,
            &crewd_proto::RemotePublicIdentity {
                authority_id: AUTH.into(),
                db_id: db_id.clone(),
                certificate_der_base64: "synthetic".into(),
                spki_sha256: "a".repeat(64),
            },
            "t",
        )
        .unwrap();
        let grant_id = uuid::Uuid::new_v4().to_string();
        c.execute("INSERT INTO remote_grants(id,authority_id,device_label,client_spki_sha256,client_certificate_der_base64,revision,created_at) VALUES(?1,?2,'Synthetic',?3,'synthetic',1,'t')",params![grant_id,AUTH,"b".repeat(64)]).unwrap();
        let owner = Owner {
            authority_id: AUTH.into(),
            db_id: db_id.clone(),
            grant_id,
            grant_revision: "1".into(),
            client_pin: "b".repeat(64),
        };
        let tx = c.unchecked_transaction().unwrap();
        configure(
            &tx,
            0,
            true,
            Some(&ProviderBinding {
                authority_id: AUTH.into(),
                db_id,
                team_id: "1234567890".into(),
                key_id: "ABCDEFGHIJ".into(),
                environment: PushEnvironment::Sandbox,
                topic: "com.pendingname.pendingcrew".into(),
                key_slot: uuid::Uuid::new_v4().to_string(),
                seal_slot: uuid::Uuid::new_v4().to_string(),
            }),
        )
        .unwrap();
        tx.commit().unwrap();
        owner
    }
    fn registration() -> NotificationRegisterParams {
        NotificationRegisterParams {
            request_id: uuid::Uuid::new_v4().to_string(),
            installation_id: uuid::Uuid::new_v4().to_string(),
            expected_generation: "0".into(),
            device_token: "01ab".into(),
            environment: PushEnvironment::Sandbox,
            topic: "com.pendingname.pendingcrew".into(),
            crew_ids: vec!["crew".into()],
            kinds: vec![
                NotificationKind::Done,
                NotificationKind::Reported,
                NotificationKind::ReplyReady,
                NotificationKind::ResponseFailed,
                NotificationKind::HumanApprovalRequired,
            ],
        }
    }
    fn install(c: &Connection, owner: &Owner, p: &NotificationRegisterParams) {
        let tx = c.unchecked_transaction().unwrap();
        register(&tx, owner, p, &[7; 32]).unwrap();
        tx.commit().unwrap();
    }
    fn event(c: &Connection, seq: u64, now: i64) {
        let tx = c.unchecked_transaction().unwrap();
        enqueue(
            &tx,
            "crew",
            NotificationKind::Done,
            NotificationSource::WorkEvent {
                event_seq: seq.to_string(),
                work_id: Some("W1".into()),
                ask_id: None,
                entry_no: Some("#1".into()),
                object_id: "W12".into(),
            },
            None,
            now,
        )
        .unwrap();
        tx.commit().unwrap();
    }
    fn claim_one(c: &Connection, now: i64) -> Option<Claimed> {
        let tx = c.unchecked_transaction().unwrap();
        let result = claim(&tx, now, settings(&tx).unwrap().revision).unwrap();
        tx.commit().unwrap();
        result
    }
    fn count(c: &Connection) -> i64 {
        c.query_row("SELECT COUNT(*) FROM notification_outbox", [], |r| r.get(0))
            .unwrap()
    }
    #[test]
    fn original_request_uuid_is_owner_bound_and_never_reapplies_unknown_or_known() {
        let s = Store::open_in_memory().unwrap();
        let owner = setup(&s);
        let id = uuid::Uuid::new_v4().to_string();
        let h = hash(b"original");
        let tx = s.conn().unchecked_transaction().unwrap();
        assert!(reserve(&tx, &owner.key(), &id, "register", &h).unwrap());
        tx.commit().unwrap();
        assert!(matches!(
            request_lookup(s.conn(), &owner.key(), &id).unwrap(),
            NotificationRequestLookup::Unknown { .. }
        ));
        let tx = s.conn().unchecked_transaction().unwrap();
        assert!(!reserve(&tx, &owner.key(), &id, "register", &h).unwrap());
        assert!(reserve(&tx, &owner.key(), &id, "register", &hash(b"new")).is_err());
        result(
            &tx,
            &owner.key(),
            &id,
            &serde_json::json!({"generation":"1"}),
        )
        .unwrap();
        tx.commit().unwrap();
        assert!(matches!(
            request_lookup(s.conn(), &owner.key(), &id).unwrap(),
            NotificationRequestLookup::Known { .. }
        ));
        assert!(request_lookup(s.conn(), "anotherGrant", &id).is_err());
        assert!(matches!(
            request_lookup(s.conn(), &owner.key(), &uuid::Uuid::new_v4().to_string()).unwrap(),
            NotificationRequestLookup::NotFound
        ));
    }
    #[test]
    fn unknown_before_io_survives_actual_close_reopen_and_rotation_without_resend() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("db");
        let owner;
        let mut p = registration();
        let attempt;
        {
            let s = Store::open(&path).unwrap();
            owner = setup(&s);
            install(s.conn(), &owner, &p);
            event(s.conn(), 1, 1000);
            let first = claim_one(s.conn(), 1001).unwrap();
            attempt = first.attempt;
            assert_eq!(first.expires_at, 86_401_000);
        }
        {
            let s = Store::open(&path).unwrap();
            assert!(claim_one(s.conn(), 1002).is_none());
            p.expected_generation = "1".into();
            install(s.conn(), &owner, &p);
            event(s.conn(), 1, 2000);
            assert_eq!(count(s.conn()), 1);
            assert!(claim_one(s.conn(), 2001).is_none());
            let tx = s.conn().unchecked_transaction().unwrap();
            settle(
                &tx,
                1,
                &attempt,
                NotificationDeliveryState::AcceptedByApns,
                None,
                false,
            )
            .unwrap();
            tx.commit().unwrap();
            assert!(claim_one(s.conn(), 2002).is_none());
        }
    }
    #[test]
    fn grant_revocation_and_subscription_cas_block_future_claims_but_do_not_withdraw_inflight() {
        let s = Store::open_in_memory().unwrap();
        let owner = setup(&s);
        let p = registration();
        install(s.conn(), &owner, &p);
        event(s.conn(), 1, 1000);
        let first = claim_one(s.conn(), 1001).unwrap();
        event(s.conn(), 2, 1000);
        assert!(
            crate::remote::revoke_grant(
                s.conn(),
                &owner.grant_id,
                &owner.authority_id,
                1,
                "explicit revoke",
                "now"
            )
            .unwrap()
        );
        assert_eq!(
            s.conn()
                .query_row(
                    "SELECT state FROM notification_outbox WHERE id=?1",
                    [first.delivery_id + 1],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "suppressed"
        );
        assert!(!still_allowed(s.conn(), &first, 1002).unwrap());
        assert!(claim_one(s.conn(), 1002).is_none());
        let tx = s.conn().unchecked_transaction().unwrap();
        settle(
            &tx,
            first.delivery_id,
            &first.attempt,
            NotificationDeliveryState::AcceptedByApns,
            None,
            false,
        )
        .unwrap();
        tx.commit().unwrap();
        let states: Vec<String> = s
            .conn()
            .prepare("SELECT state FROM notification_outbox ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(states, vec!["accepted", "suppressed"]);
    }
    #[test]
    fn late_invalid_token_cannot_revoke_rotated_generation_and_tombstone_cannot_resurrect() {
        let s = Store::open_in_memory().unwrap();
        let owner = setup(&s);
        let mut p = registration();
        install(s.conn(), &owner, &p);
        event(s.conn(), 1, 1000);
        let first = claim_one(s.conn(), 1001).unwrap();
        p.expected_generation = "1".into();
        install(s.conn(), &owner, &p);
        let tx = s.conn().unchecked_transaction().unwrap();
        settle(
            &tx,
            first.delivery_id,
            &first.attempt,
            NotificationDeliveryState::Rejected,
            Some("Unregistered"),
            true,
        )
        .unwrap();
        tx.commit().unwrap();
        assert!(
            !installation(s.conn(), &owner, &p.installation_id)
                .unwrap()
                .unwrap()
                .revoked
        );
        let tx = s.conn().unchecked_transaction().unwrap();
        let revoked = revoke(&tx, &owner, &p.installation_id, 2).unwrap();
        assert_eq!(revoked.generation, "3");
        tx.commit().unwrap();
        p.expected_generation = "3".into();
        let tx = s.conn().unchecked_transaction().unwrap();
        assert!(register(&tx, &owner, &p, &[8; 32]).is_err());
        tx.rollback().unwrap();
    }
    #[test]
    fn immutable_24h_expiry_is_not_extended_by_rotation_replay_and_restore() {
        let s = Store::open_in_memory().unwrap();
        let owner = setup(&s);
        let mut p = registration();
        install(s.conn(), &owner, &p);
        event(s.conn(), 1, 1000);
        p.expected_generation = "1".into();
        install(s.conn(), &owner, &p);
        event(s.conn(), 1, 20_000);
        assert!(claim_one(s.conn(), 86_401_000).is_none());
        event(s.conn(), 2, 86_402_000);
        let tx = s.conn().unchecked_transaction().unwrap();
        restored(&tx).unwrap();
        tx.commit().unwrap();
        assert!(!settings(s.conn()).unwrap().enabled);
        assert!(claim_one(s.conn(), 86_402_001).is_none());
        assert!(
            installation(s.conn(), &owner, &p.installation_id)
                .unwrap()
                .unwrap()
                .revoked
        );
    }
    #[test]
    fn reply_revision_and_kind_changes_cannot_create_second_original_source_attempt() {
        let s = Store::open_in_memory().unwrap();
        let owner = setup(&s);
        let p = registration();
        install(s.conn(), &owner, &p);
        for (kind, revision) in [
            (NotificationKind::ReplyReady, "2"),
            (NotificationKind::ResponseFailed, "3"),
        ] {
            let tx = s.conn().unchecked_transaction().unwrap();
            enqueue(
                &tx,
                "crew",
                kind,
                NotificationSource::ManagedResponse {
                    session_id: "session".into(),
                    runtime_id: "runtime".into(),
                    request_id: "request".into(),
                    prompt_id: "prompt".into(),
                    receipt_revision: revision.into(),
                },
                Some(("session", "generation")),
                1000,
            )
            .unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(count(s.conn()), 1);
        let tx = s.conn().unchecked_transaction().unwrap();
        suppress_session(&tx, "session", "resumed").unwrap();
        tx.commit().unwrap();
        assert!(claim_one(s.conn(), 1001).is_none());
    }
    #[test]
    fn zero_configuration_or_no_subscription_produces_no_observation_rows() {
        let s = Store::open_in_memory().unwrap();
        event(s.conn(), 1, 1000);
        assert_eq!(count(s.conn()), 0);
        let _owner = setup(&s);
        event(s.conn(), 2, 1000);
        assert_eq!(count(s.conn()), 0);
        let tx = s.conn().unchecked_transaction().unwrap();
        let no_subscriber = crewd_work::Event {
            seq: 3,
            at: crewd_work::Ts(1001),
            by: crewd_work::Actor::Human,
            kind: crewd_work::EventKind::Done {
                id: crewd_work::WorkId(999),
                evidence: None,
            },
        };
        enqueue_work(&tx, "crew", &no_subscriber).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            count(s.conn()),
            0,
            "no subscribers must not resolve or invent public source identities"
        );
    }
    #[test]
    fn managed_reply_original_receipt_and_outbox_commit_rollback_and_retraction_are_one_transaction()
     {
        use crate::native_execution as n;
        let s = Store::open_in_memory().unwrap();
        let owner = setup(&s);
        let p = registration();
        install(s.conn(), &owner, &p);
        s.conn().execute("INSERT INTO sessions(id,crew_id,role,executor,updated_at,desired,was_working)VALUES('session','crew','worker','claude','t','running',0)",[]).unwrap();
        let runtime = n::Runtime {
            id: uuid::Uuid::new_v4().to_string(),
            session: "session".into(),
            generation: uuid::Uuid::new_v4().to_string(),
            epoch: 1,
            native_pid: 42,
            native_start: 123,
            native_boot: "synthetic".into(),
        };
        let tx = s.conn().unchecked_transaction().unwrap();
        let seal = n::register_managed(&tx, &runtime).unwrap();
        tx.commit().unwrap();
        let request = uuid::Uuid::new_v4().to_string();
        let prompt = uuid::Uuid::new_v4().to_string();
        let tx = s.conn().unchecked_transaction().unwrap();
        n::prepare(&tx, &runtime.id, &request, b"body", "t").unwrap();
        let accepted =
            n::managed_accept_content(&tx, &seal, &request, 0, &prompt, b"body", "t").unwrap();
        tx.commit().unwrap();
        {
            let tx = s.conn().unchecked_transaction().unwrap();
            let waiting =
                n::managed_response_waiting(&tx, &seal, &request, accepted.revision, &prompt, "t")
                    .unwrap();
            assert!(waiting.turn_id.is_none());
            assert_eq!(count(&tx), 1);
        }
        assert_eq!(count(s.conn()), 0);
        assert_eq!(
            n::get(s.conn(), &request).unwrap().unwrap().phase,
            n::Phase::ContentAccepted
        );
        let tx = s.conn().unchecked_transaction().unwrap();
        n::managed_response_waiting(&tx, &seal, &request, accepted.revision, &prompt, "t").unwrap();
        tx.commit().unwrap();
        assert_eq!(count(s.conn()), 1);
        let tx = s.conn().unchecked_transaction().unwrap();
        n::managed_response_resumed(&tx, &seal, &prompt, "t").unwrap();
        tx.commit().unwrap();
        assert!(claim_one(s.conn(), crate::now_ms()).is_none());
        assert_eq!(
            s.conn()
                .query_row("SELECT state FROM notification_outbox", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "suppressed"
        );
    }

    #[test]
    fn exact_delivery_get_is_readonly_unknown_survives_rotation_and_revoked_hint_is_unavailable() {
        let s = Store::open_in_memory().unwrap();
        let owner = setup(&s);
        let mut p = registration();
        install(s.conn(), &owner, &p);
        event(s.conn(), 1, 1000);
        let claim = claim_one(s.conn(), 1001).unwrap();
        let query = NotificationDeliveryGetParams {
            installation_id: p.installation_id.clone(),
            delivery_id: claim.delivery_id.to_string(),
        };
        let first = delivery_get(s.conn(), &owner, &query).unwrap().unwrap();
        assert_eq!(first.state, NotificationDeliveryState::Unknown);
        assert_eq!(first.apns_id.as_deref(), Some(claim.attempt.as_str()));
        p.expected_generation = "1".into();
        install(s.conn(), &owner, &p);
        let rotated = delivery_get(s.conn(), &owner, &query).unwrap().unwrap();
        assert_eq!(first.context, rotated.context);
        assert_eq!(rotated.state, NotificationDeliveryState::Unknown);
        assert_eq!(first.apns_id, rotated.apns_id);
        assert!(
            delivery_get(
                s.conn(),
                &owner,
                &NotificationDeliveryGetParams {
                    delivery_id: "99999".into(),
                    ..query.clone()
                }
            )
            .unwrap()
            .is_none()
        );
        let tx = s.conn().unchecked_transaction().unwrap();
        revoke(&tx, &owner, &p.installation_id, 2).unwrap();
        tx.commit().unwrap();
        assert!(delivery_get(s.conn(), &owner, &query).unwrap().is_none());
        assert_eq!(
            s.conn()
                .query_row("SELECT state FROM notification_outbox", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "unknown"
        );
    }
    #[test]
    fn original_restore_reuses_numeric_delivery_but_old_installation_hint_cannot_navigate_new_grant()
     {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().canonicalize().unwrap();
        let backup = path.join("prior.db");
        let s = Store::open_data_dir(&path).unwrap();
        let owner = setup(&s);
        let p = registration();
        install(s.conn(), &owner, &p);
        drop(s);
        crate::backup::backup_db(&path.join(crate::DB_FILE), &backup).unwrap();
        let s = Store::open_data_dir(&path).unwrap();
        event(s.conn(), 1, 1000);
        let original = claim_one(s.conn(), 1001).unwrap();
        let old = NotificationDeliveryGetParams {
            installation_id: p.installation_id.clone(),
            delivery_id: original.delivery_id.to_string(),
        };
        drop(s);
        crate::backup::restore(&path, &backup).unwrap();
        let s = Store::open_data_dir(&path).unwrap();
        assert!(owner.authorize(s.conn()).is_err());
        assert!(!settings(s.conn()).unwrap().enabled);
        let mut fresh = owner.clone();
        fresh.grant_id = uuid::Uuid::new_v4().to_string();
        fresh.client_pin = "c".repeat(64);
        s.conn()
            .execute_batch("UPDATE remote_configuration SET enabled=1")
            .unwrap();
        s.conn().execute("INSERT INTO remote_grants(id,authority_id,device_label,client_spki_sha256,client_certificate_der_base64,revision,created_at)VALUES(?1,?2,'new explicit pairing',?3,'synthetic',1,'t')",params![fresh.grant_id,fresh.authority_id,fresh.client_pin]).unwrap();
        let settings = settings(s.conn()).unwrap();
        let tx = s.conn().unchecked_transaction().unwrap();
        configure(&tx, settings.revision, true, settings.binding.as_ref()).unwrap();
        tx.commit().unwrap();
        let mut new = p.clone();
        new.installation_id = uuid::Uuid::new_v4().to_string();
        install(s.conn(), &fresh, &new);
        let tx = s.conn().unchecked_transaction().unwrap();
        enqueue(
            &tx,
            "crew",
            NotificationKind::Done,
            NotificationSource::WorkEvent {
                event_seq: "1".into(),
                work_id: Some("W999".into()),
                ask_id: None,
                entry_no: None,
                object_id: "W999".into(),
            },
            None,
            2000,
        )
        .unwrap();
        tx.commit().unwrap();
        let replacement = claim_one(s.conn(), 2001).unwrap();
        assert_eq!(
            replacement.delivery_id, original.delivery_id,
            "original snapshot genuinely rewinds SQLite delivery sequence"
        );
        assert_ne!(replacement.context, original.context);
        assert!(
            delivery_get(s.conn(), &fresh, &old).is_err(),
            "new verified grant cannot use the original installation even when numeric IDs coincide"
        );
        let mut substituted = old;
        substituted.installation_id = new.installation_id;
        let changed = delivery_get(s.conn(), &fresh, &substituted)
            .unwrap()
            .unwrap();
        assert_ne!(
            changed.context, original.context,
            "exact context equality also rejects a caller substituting its new installation"
        );
    }

    #[test]
    fn another_live_grant_cannot_take_installation_and_current_crew_or_provider_change_denies_claim()
     {
        let s = Store::open_in_memory().unwrap();
        let owner = setup(&s);
        let p = registration();
        install(s.conn(), &owner, &p);
        let mut foreign = owner.clone();
        foreign.grant_id = uuid::Uuid::new_v4().to_string();
        foreign.client_pin = "c".repeat(64);
        s.conn().execute("INSERT INTO remote_grants(id,authority_id,device_label,client_spki_sha256,client_certificate_der_base64,revision,created_at) VALUES(?1,?2,'other',?3,'synthetic',1,'t')",params![foreign.grant_id,foreign.authority_id,foreign.client_pin]).unwrap();
        assert!(foreign.authorize(s.conn()).is_ok());
        assert!(installation(s.conn(), &foreign, &p.installation_id).is_err());
        let tx = s.conn().unchecked_transaction().unwrap();
        let mut takeover = p.clone();
        takeover.expected_generation = "1".into();
        assert!(register(&tx, &foreign, &takeover, &[8; 32]).is_err());
        tx.rollback().unwrap();
        event(s.conn(), 1, 1000);
        s.conn()
            .execute("UPDATE crews SET archived_at='now' WHERE id='crew'", [])
            .unwrap();
        assert!(claim_one(s.conn(), 1001).is_none());
        s.conn()
            .execute("UPDATE crews SET archived_at=NULL WHERE id='crew'", [])
            .unwrap();
        event(s.conn(), 2, 1002);
        let settings = settings(s.conn()).unwrap();
        let tx = s.conn().unchecked_transaction().unwrap();
        configure(&tx, settings.revision, false, settings.binding.as_ref()).unwrap();
        tx.commit().unwrap();
        assert!(claim_one(s.conn(), 1003).is_none());
        assert_eq!(
            s.conn()
                .query_row(
                    "SELECT COUNT(*) FROM notification_outbox WHERE state='suppressed'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
    }

    #[test]
    fn original_workbook_kinds_and_ask_resolution_keep_real_source_identity() {
        use crate::object_refs::ObjectKind;
        use crewd_work::{
            Actor, BlockOn, Config, Continuation, CrewId, NewItem, Report, Ts, WorkBook,
        };
        let s = Store::open_in_memory().unwrap();
        let owner = setup(&s);
        let mut p = registration();
        p.kinds = vec![
            NotificationKind::Done,
            NotificationKind::Reported,
            NotificationKind::Blocked,
            NotificationKind::AskOpened,
        ];
        install(s.conn(), &owner, &p);
        let tx = s.conn().unchecked_transaction().unwrap();
        let mut book = WorkBook::restore(
            crate::work_organization::load(&tx).unwrap().unwrap(),
            Config::default(),
        )
        .unwrap();
        let mut input = NewItem::titled("Synthetic source");
        input.owner = Some(crewd_work::SessionId::new("synthetic-owner"));
        input.crew_id = Some(CrewId::new("crew"));
        let now = crate::now_ms();
        let made = book.create_item(&Actor::Human, Ts(now), input).unwrap();
        let public = crate::work_organization::bind_core(
            &tx,
            ObjectKind::Plan,
            "crew",
            &made.value.to_string(),
            "t",
        )
        .unwrap();
        crate::work_organization::save(&tx, book.state(), &made.events, "t").unwrap();
        let report = book
            .report(
                &Actor::Human,
                Ts(now + 1),
                made.value,
                Report {
                    status: None,
                    next_check_at: None,
                    note: "progress".into(),
                },
            )
            .unwrap();
        crate::work_organization::save(&tx, book.state(), &report.events, "t").unwrap();
        let blocked = book
            .block(
                &Actor::Human,
                Ts(now + 2),
                made.value,
                vec![BlockOn::NewAsk {
                    question: "Need Human".into(),
                    default: None,
                }],
                Continuation::Nothing,
            )
            .unwrap();
        let ask = blocked.value.asks[0];
        crate::work_organization::bind_core(
            &tx,
            ObjectKind::HumanTask,
            "crew",
            &ask.to_string(),
            "t",
        )
        .unwrap();
        crate::work_organization::save(&tx, book.state(), &blocked.events, "t").unwrap();
        let answered = book
            .answer_ask(&Actor::Human, Ts(now + 3), ask, "answer")
            .unwrap();
        crate::work_organization::save(&tx, book.state(), &answered.events, "t").unwrap();
        let done = book
            .mark_done(&Actor::Human, Ts(now + 4), made.value, None)
            .unwrap();
        crate::work_organization::save(&tx, book.state(), &done.events, "t").unwrap();
        tx.commit().unwrap();
        let page = deliveries(
            s.conn(),
            &owner,
            &NotificationDeliveryListParams {
                installation_id: p.installation_id,
                after_delivery_id: None,
                limit: 100,
            },
        )
        .unwrap();
        assert_eq!(page.items.len(), 4);
        for row in page.items {
            let NotificationSource::WorkEvent {
                event_seq,
                work_id,
                ask_id,
                entry_no,
                object_id,
            } = row.context.source
            else {
                panic!("must preserve source tag")
            };
            assert!(number(&event_seq, false).is_ok());
            assert_eq!(work_id, Some(made.value.to_string()));
            assert_eq!(entry_no, None);
            if row.context.kind == NotificationKind::AskOpened {
                assert_eq!(ask_id, Some(ask.to_string()));
                assert_eq!(row.state, NotificationDeliveryState::Suppressed)
            } else {
                assert_eq!(object_id, public.id());
                assert_eq!(row.state, NotificationDeliveryState::Pending)
            }
        }
    }

    #[test]
    fn drop_transaction_discards_notification_and_work_state_together() {
        use crate::object_refs::ObjectKind;
        use crewd_work::{Actor, Config, CrewId, NewItem, Report, Ts, WorkBook};
        let s = Store::open_in_memory().unwrap();
        let owner = setup(&s);
        let p = registration();
        install(s.conn(), &owner, &p);
        let tx = s.conn().unchecked_transaction().unwrap();
        let mut book = WorkBook::restore(
            crate::work_organization::load(&tx).unwrap().unwrap(),
            Config::default(),
        )
        .unwrap();
        let mut n = NewItem::titled("Synthetic");
        n.crew_id = Some(CrewId::new("crew"));
        let made = book.create_item(&Actor::Human, Ts(1000), n).unwrap();
        crate::work_organization::bind_core(
            &tx,
            ObjectKind::Plan,
            "crew",
            &made.value.to_string(),
            "t",
        )
        .unwrap();
        crate::work_organization::save(&tx, book.state(), &made.events, "t").unwrap();
        tx.commit().unwrap();
        let before = crate::work_organization::load(s.conn()).unwrap().unwrap();
        {
            let tx = s.conn().unchecked_transaction().unwrap();
            let mut book = WorkBook::restore(before.clone(), Config::default()).unwrap();
            let reported = book
                .report(
                    &Actor::Human,
                    Ts(1001),
                    made.value,
                    Report {
                        status: None,
                        next_check_at: None,
                        note: "Synthetic progress".into(),
                    },
                )
                .unwrap();
            crate::work_organization::save(&tx, book.state(), &reported.events, "t").unwrap();
            assert_eq!(count(&tx), 1);
        }
        assert_eq!(count(s.conn()), 0);
        assert_eq!(
            crate::work_organization::load(s.conn()).unwrap().unwrap(),
            before
        );
    }
}
