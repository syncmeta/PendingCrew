//! Test-only real collab sidecar for shared causal contracts.
//! Legacy Mem rows enter through a substrate mirror; new causal posts use actual collab Tx.
use super::{MemKind, MemState};
use crate::causal_fence::CausalFence;
use anyhow::Result;
use crewd_collab::{self as collab, causal, cursor_reservations as cr};
use crewd_store::Store;
use std::collections::BTreeMap;
use std::sync::Arc;

pub(super) struct MemCausal {
    pub store: Store,
    pub enabled: bool,
    pub fence: Arc<CausalFence>,
    /// Actual Mem channel/seq -> actual sidecar Message. No global-seq assumption.
    messages: BTreeMap<(String, i64), crewd_proto::Message>,
    /// Real generated message -> Mem channel/seq, including original fixture identity.
    reverse: BTreeMap<String, (String, i64, String)>,
    notices: std::collections::BTreeSet<String>,
}
impl MemCausal {
    pub fn new() -> Result<Self> {
        Ok(Self {
            store: Store::open_in_memory()?,
            enabled: false,
            fence: Arc::new(CausalFence::default()),
            messages: BTreeMap::new(),
            reverse: BTreeMap::new(),
            notices: Default::default(),
        })
    }
    pub fn context(&self) -> collab::TxContext {
        collab::TxContext::new(self.fence.clone())
    }
    /// Fixture imports do not create causal stamps. The shared reader's documented legacy-root
    /// contract applies; only post() below can create a new inherited causal message.
    pub fn sync(&mut self, st: &MemState) -> Result<()> {
        let mut imported = self.messages.clone();
        let mut reverse = self.reverse.clone();
        let c = self.store.conn();
        c.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<()> {
            for (crew, parent) in &st.crew_parents {
                for id in [crew, parent] {
                    c.execute("INSERT OR IGNORE INTO crews(id,title,created_at,updated_at) VALUES(?1,?1,'1970-01-01T00:00:00Z','1970-01-01T00:00:00Z')",[id])?;
                }
            }
            for row in st.sessions.values() {
                c.execute("INSERT OR IGNORE INTO crews(id,title,created_at,updated_at) VALUES(?1,?1,'1970-01-01T00:00:00Z','1970-01-01T00:00:00Z')",[&row.crew_id])?;
                c.execute(
                    "UPDATE crews SET archived_at=?2 WHERE id=?1",
                    rusqlite::params![
                        row.crew_id,
                        st.removed_crews
                            .contains(&row.crew_id)
                            .then_some("1970-01-01T00:00:00Z")
                    ],
                )?;
                crewd_store::sessions::upsert(
                    c,
                    &crewd_proto::Session {
                        id: row.id.clone(),
                        crew_id: row.crew_id.clone(),
                        role: if row.captain {
                            crewd_proto::Role::Captain
                        } else {
                            crewd_proto::Role::Worker
                        },
                        executor: row.executor.clone(),
                        display_name: row.display_name.clone(),
                        desired: match row.desired {
                            crate::orchestra::port::Desired::Running => {
                                crewd_proto::Desired::Running
                            }
                            crate::orchestra::port::Desired::Stopped => {
                                crewd_proto::Desired::Stopped
                            }
                        },
                        updated_at: "1970-01-01T00:00:00Z".into(),
                        ..Default::default()
                    },
                    None,
                )?;
                if row.captain {
                    c.execute(
                        "UPDATE crews SET captain_session_id=?2 WHERE id=?1",
                        rusqlite::params![row.crew_id, row.id],
                    )?;
                }
            }
            c.execute("DELETE FROM crew_parents", [])?;
            for (crew, parent) in &st.crew_parents {
                c.execute(
                    "INSERT INTO crew_parents(crew_id,parent_id) VALUES(?1,?2)",
                    rusqlite::params![crew, parent],
                )?;
            }
            for (channel, ch) in &st.channels {
                let kind = match &ch.kind {
                    MemKind::Lobby => "lobby",
                    MemKind::Human => "human",
                    MemKind::Dm => "dm",
                    MemKind::Other => "topic",
                };
                c.execute("INSERT OR IGNORE INTO channels(id,crew_id,kind,name,created_by,created_at) VALUES(?1,?2,?3,?1,'system','1970-01-01T00:00:00Z')",rusqlite::params![channel,ch.crew,kind])?;
                c.execute(
                    "UPDATE channels SET archived_at=?2 WHERE id=?1",
                    rusqlite::params![
                        channel,
                        st.archived_channels
                            .contains(channel)
                            .then_some("1970-01-01T00:00:00Z")
                    ],
                )?;
                c.execute("DELETE FROM channel_members WHERE channel_id=?1", [channel])?;
                let members = super::MemPort::ctx_of(st, channel, st.now)
                    .map(|ctx| ctx.session_members)
                    .unwrap_or_default();
                for member in members {
                    c.execute("INSERT INTO channel_members(channel_id,member_kind,member_id,joined_at) VALUES(?1,'session',?2,'1970-01-01T00:00:00Z')",rusqlite::params![channel,member])?;
                }
            }
            c.execute("DELETE FROM listens", [])?;
            for (channel, listens) in &st.listens {
                for l in listens {
                    c.execute("INSERT INTO listens(session_id,channel_id,until,senders,created_at) VALUES(?1,?2,?3,?4,'1970-01-01T00:00:00Z')",rusqlite::params![l.session,channel,l.until.0,serde_json::to_string(&l.senders.clone().unwrap_or_default())?])?;
                }
            }
            // Pins imported after their canonical message prefixes below. An absent
            // pin remains absent; a nonzero unrepresented prefix fails closed.
            c.execute("DELETE FROM read_cursors", [])?;
            // Import only immutable fixture rows; canonical alias names include full tuple.
            // New real posts are already in messages/reverse and are never reimported or restamped.
            for ((channel, seq), (facts, text)) in &st.messages {
                let key = (channel.clone(), *seq);
                if let Some(existing) = imported.get(&key) {
                    if existing.text != *text {
                        anyhow::ensure!(
                            !self.enabled,
                            "mutated immutable new causal message tuple"
                        );
                        c.execute(
                            "UPDATE messages SET text=?2 WHERE id=?1",
                            rusqlite::params![existing.id, text],
                        )?;
                        let mut updated = existing.clone();
                        updated.text = text.clone();
                        imported.insert(key, updated);
                    }
                    continue;
                }
                let (kind, sender) = match &facts.sender {
                    crewd_wake::plan::Sender::Human => (crewd_proto::SenderKind::Human, None),
                    crewd_wake::plan::Sender::System => (crewd_proto::SenderKind::System, None),
                    crewd_wake::plan::Sender::Session { id, captain } => (
                        if *captain {
                            crewd_proto::SenderKind::Captain
                        } else {
                            crewd_proto::SenderKind::Session
                        },
                        Some(id.clone()),
                    ),
                };
                // Legacy synthetic MsgFacts can carry a StopRelay plan tag; do not create
                // a private trusted marker from it. New actual posts use strict shared read.
                anyhow::ensure!(
                    !self.enabled
                        || facts.provenance == crewd_wake::plan::MessageProvenance::Ordinary,
                    "new StopRelay requires the genuine relay writer, not a forged fixture marker"
                );
                let id = format!("mem:{}:{}:{}", channel, seq, facts.id);
                let mut message = crewd_proto::Message {
                    id,
                    channel_id: channel.clone(),
                    sender_kind: kind,
                    sender_session_id: sender,
                    text: text.clone(),
                    created_at: "1970-01-01T00:00:00Z".into(),
                    mentions: facts
                        .mentions
                        .iter()
                        .map(|m| match m {
                            crewd_wake::plan::Mention::Human => crewd_proto::Mention {
                                kind: crewd_proto::MentionKind::Human,
                                target_id: None,
                            },
                            crewd_wake::plan::Mention::Session(id) => crewd_proto::Mention {
                                kind: crewd_proto::MentionKind::Session,
                                target_id: Some(id.clone()),
                            },
                        })
                        .collect(),
                    ..Default::default()
                };
                message.seq = crewd_store::messages::insert(self.store.conn(), &message, None)?
                    .expect("new canonical fixture tuple");
                reverse.insert(
                    message.id.clone(),
                    (channel.clone(), *seq, facts.id.clone()),
                );
                imported.insert(key, message);
            }
            // Cursor values are remapped after import; initial zero is an actual existing pin only.
            for ((session, channel), seq) in &st.cursors {
                let mapped = match Self::mapped(&imported, channel, *seq) {
                    Ok(mapped) => mapped,
                    Err(_) if !self.enabled => continue, // honest legacy unknown surface, NOT cursor0
                    Err(error) => return Err(error),
                };
                self.store.conn().execute("INSERT INTO read_cursors(reader,channel_id,seq,updated_at) VALUES(?1,?2,?3,'1970-01-01T00:00:00Z')",rusqlite::params![session,channel,mapped])?;
            }
            for row in &st.wakes {
                let source = match &row.msg {
                    Some(m) => imported
                        .get(&(m.channel.clone(), m.seq))
                        .map(|stored| collab::wakes::source::message(&stored.id))
                        .unwrap_or_else(|| row.source.clone()),
                    None => row.source.clone(),
                };
                c.execute("INSERT INTO wakes(id,session_id,kind,source,origin,priority,not_before,note,state,attempts,last_error,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'1970-01-01T00:00:00Z') ON CONFLICT(id) DO UPDATE SET state=excluded.state,attempts=excluded.attempts,last_error=excluded.last_error,not_before=excluded.not_before",rusqlite::params![row.id,row.session,row.kind.as_str(),source,row.origin,row.priority.as_str(),row.not_before.0,row.note,row.state.as_str(),row.attempts,row.last_error])?;
            }
            // Preserve original durable cancellation floor, including legacy fixtures that
            // become real-post models later. Floor import never drops newer rows or releases owner.
            for (session, floor) in &st.human_stop_floors {
                let key = format!("human_stop_wake_floor:{session}");
                let stored = crewd_store::meta_get(c, &key)?
                    .map(|v| v.parse::<i64>())
                    .transpose()?;
                crewd_store::meta_set(c, &key, &stored.unwrap_or(0).max(*floor).to_string())?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => c.execute_batch("COMMIT")?,
            Err(error) => {
                c.execute_batch("ROLLBACK")?;
                return Err(error);
            }
        }
        self.messages = imported;
        self.reverse = reverse;
        Ok(())
    }
    fn mapped(
        messages: &BTreeMap<(String, i64), crewd_proto::Message>,
        channel: &str,
        seq: i64,
    ) -> Result<i64> {
        if seq == 0 {
            return Ok(0);
        }
        messages
            .range((channel.to_owned(), i64::MIN)..=(channel.to_owned(), seq))
            .next_back()
            .map(|(_, m)| m.seq)
            .ok_or_else(|| {
                anyhow::anyhow!("nonzero Mem pin has no canonical message prefix: {channel}#{seq}")
            })
    }
    fn map_seq(&self, channel: &str, seq: i64) -> Result<i64> {
        if seq == 0 {
            return Ok(0);
        }
        Self::mapped(&self.messages, channel, seq)
    }
    pub fn targets(&self, targets: &[(String, i64)]) -> Result<Vec<(String, i64)>> {
        targets
            .iter()
            .map(|(channel, seq)| Ok((channel.clone(), self.map_seq(channel, *seq)?)))
            .collect()
    }
    pub fn diagnostics(&self, session: &str, now: i64) -> Result<causal::Admission> {
        Ok(causal::diagnostics(self.store.conn(), session, now)?)
    }
    pub fn prepare_memory(&mut self, st: &MemState, session: &str, now: i64) -> Result<MemState> {
        let session = session.to_owned();
        let context = self.context();
        let notices = self.notices.clone();
        let messages = self.messages.clone();
        let enabled = self.enabled;
        let ((next, new_seen), _) =
            collab::run_with_context(&mut self.store, now, context, move |tx| {
                let before: i64 =
                    tx.conn()
                        .query_row("SELECT COALESCE(MAX(seq),0) FROM messages", [], |r| {
                            r.get(0)
                        })?;
                let prepared = cr::prepare_causal_unread(tx, &session)?;
                crate::causal_fence::blocked_channels(tx, &prepared.causes)?;
                let mut next = st.clone();
                let mut new_seen = notices.clone();
                let mut q = tx
                    .conn()
                    .prepare("SELECT reader,channel_id,seq FROM read_cursors")?;
                for row in q.query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                })? {
                    let (reader, channel, seq) = row?;
                    // enabled model's new real posts use actual global seq as Mem seq.
                    let local = messages
                        .iter()
                        .filter(|((ch, _), m)| ch == &channel && m.seq <= seq)
                        .max_by_key(|(_, m)| m.seq)
                        .map(|((_, local), _)| *local)
                        .unwrap_or(0);
                    if enabled {
                        next.cursors.insert((reader, channel), local);
                    }
                }
                let mut q = tx
                    .conn()
                    .prepare("SELECT id,crew_id FROM channels WHERE kind='human'")?;
                for row in
                    q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                {
                    let (channel, crew) = row?;
                    for m in crewd_collab::messages::list(tx.conn(), &channel, None, None, 10000)? {
                        if m.seq <= before || !new_seen.insert(m.id.clone()) {
                            continue;
                        }
                        next.posts.push((
                            super::Audience::HumanChannel {
                                crew: crew.clone(),
                                mention_human: true,
                            },
                            m.text.clone(),
                        ));
                        next.posted.push((channel.clone(), m.text));
                        next.posted_mentions.push(vec!["human".into()]);
                    }
                }
                Ok((next, new_seen))
            })?;
        // Record identities without another SQL step by home-channel text already in the projection.
        self.notices = new_seen;
        Ok(next)
    }
    /// Real shared renderer and prepared causal/reservation snapshot in one Tx.
    /// SQLite global coordinates are translated only through captured immutable aliases.
    pub fn render_memory(
        &mut self,
        st: &MemState,
        session: &str,
        batch: &[crewd_wake::QueuedWake],
        now: i64,
    ) -> Result<(MemState, super::Rendered)> {
        let session = session.to_owned();
        let ids = batch.iter().map(|w| w.id).collect::<Vec<_>>();
        let messages = self.messages.clone();
        let notices = self.notices.clone();
        let context = self.context();
        let ((next, rendered, new_seen), _) =
            collab::run_with_context(&mut self.store, now, context, move |tx| {
                let before: i64 =
                    tx.conn()
                        .query_row("SELECT COALESCE(MAX(seq),0) FROM messages", [], |r| {
                            r.get(0)
                        })?;
                let captured = cr::prepare_causal_unread(tx, &session)?;
                let blocked = crate::causal_fence::blocked_channels(tx, &captured.causes)?;
                let mut rows = collab::wakes::by_ids(tx.conn(), &ids)?;
                rows.sort_by_key(|r| ids.iter().position(|id| *id == r.id));
                let mut allowed = Vec::new();
                for row in rows {
                    let blocked_message = match row.source.strip_prefix("msg:") {
                        Some(id) => crewd_store::messages::get(tx.conn(), id)?
                            .is_some_and(|m| blocked.contains(&m.channel_id)),
                        None => false,
                    };
                    if !blocked_message {
                        allowed.push(row);
                    }
                }
                let mut rows = allowed;
                let locale = collab::settings::locale(tx.conn())?;
                let original =
                    collab::render::build_batch(tx.conn(), &session, locale, &rows, now)?;
                let mut already = Vec::new();
                for row in &rows {
                    if row.state != collab::wakes::WakeState::Pending
                        || row.kind != collab::wakes::WakeKind::Message
                        || original.unusable.iter().any(|(id, _)| *id == row.id)
                    {
                        continue;
                    }
                    if let Some(id) = row.source.strip_prefix("msg:")
                        && let Some(m) = crewd_store::messages::get(tx.conn(), id)?
                        && collab::cursors::cursor(tx.conn(), &session, &m.channel_id)?
                            .is_some_and(|base| base >= m.seq)
                    {
                        already.push(row.id);
                    }
                }
                if !already.is_empty() {
                    collab::wakes::mark_delivered(tx, &already)?;
                }
                rows.retain(|r| !already.contains(&r.id));
                let rendered =
                    collab::render::build_batch(tx.conn(), &session, locale, &rows, now)?;
                for (id, why) in &rendered.unusable {
                    collab::wakes::mark_dropped(tx, *id, why)?;
                }
                let actual_targets = rendered.cursor_targets();
                let channels = actual_targets
                    .iter()
                    .map(|(ch, _)| ch.clone())
                    .collect::<Vec<_>>();
                let snapshot = cr::snapshot(tx.conn(), &session, &channels, now)?;
                let causes = captured.causes.select(&actual_targets)?;
                let targets = actual_targets
                    .iter()
                    .map(|(ch, seq)| {
                        messages
                            .iter()
                            .find(|((channel, _), m)| channel == ch && m.seq == *seq)
                            .map(|((_, local), _)| (ch.clone(), *local))
                            .ok_or_else(|| {
                                crewd_store::StoreError::Invalid(
                                    "render target has no immutable Mem alias".into(),
                                )
                            })
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let mut next = Self::project(tx, st)?;
                let mut q = tx
                    .conn()
                    .prepare("SELECT reader,channel_id,seq FROM read_cursors")?;
                for row in q.query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                })? {
                    let (reader, channel, seq) = row?;
                    let local = messages
                        .iter()
                        .filter(|((ch, _), m)| ch == &channel && m.seq <= seq)
                        .max_by_key(|(_, m)| m.seq)
                        .map(|((_, local), _)| *local)
                        .unwrap_or(0);
                    next.cursors.insert((reader, channel), local);
                }
                let mut new_seen = notices.clone();
                let mut q = tx
                    .conn()
                    .prepare("SELECT id,crew_id FROM channels WHERE kind='human'")?;
                for row in
                    q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                {
                    let (channel, crew) = row?;
                    for m in collab::messages::list(tx.conn(), &channel, None, None, 10000)? {
                        if m.seq <= before || !new_seen.insert(m.id.clone()) {
                            continue;
                        }
                        next.posts.push((
                            super::Audience::HumanChannel {
                                crew: crew.clone(),
                                mention_human: true,
                            },
                            m.text.clone(),
                        ));
                        next.posted.push((channel.clone(), m.text));
                        next.posted_mentions.push(vec!["human".into()]);
                    }
                }
                let mut prepared = super::memory_preparation(&next, &session, &targets);
                if let super::PreparedSnapshot::Memory { causal, .. } = &mut prepared {
                    *causal = Some(Box::new((snapshot, causes)));
                }
                let mut dropped = already;
                dropped.extend(rendered.unusable.iter().map(|(id, _)| *id));
                next.rendered_windows
                    .push((ids.clone(), rendered.deferred.clone()));
                Ok((
                    next,
                    super::Rendered {
                        text: collab::render::wake_text(locale, &rendered),
                        dropped,
                        deferred: rendered.deferred,
                        cursor_targets: targets,
                        prepared: Some(prepared),
                    },
                    new_seen,
                ))
            })?;
        self.notices = new_seen;
        Ok((next, rendered))
    }
    pub fn holds(&self, target: Option<&str>) -> Result<Vec<causal::ChainHold>> {
        Ok(causal::holds(self.store.conn(), target)?)
    }
    pub fn recover(
        &mut self,
        chain: &str,
        from: &str,
        target: &str,
        action: &causal::HumanRecoveryAction,
        now: i64,
    ) -> Result<bool> {
        let context = self.context();
        Ok(
            collab::run_with_context(&mut self.store, now, context, |tx| {
                causal::recover(tx, chain, from, target, action)
            })?
            .0,
        )
    }
    /// New model posts go through the actual stamp/observations/Hub-change-producing transaction.
    pub fn post_memory(
        &mut self,
        st: &MemState,
        sender: collab::Sender,
        post: collab::messages::NewPost,
        now: i64,
    ) -> Result<(crewd_proto::Message, MemState, Vec<collab::Change>)> {
        // A legacy fixture may expose an intentionally unknown historical pin to old
        // pure tests. Starting a NEW real causal post requires representable prefixes;
        // never silently qualify that unknown surface by assigning a free new root.
        for ((_, channel), seq) in &st.cursors {
            Self::mapped(&self.messages, channel, *seq)?;
        }
        let context = self.context();
        let existing = self.messages.clone();
        let reverse = self.reverse.clone();
        // Independent monotonically increasing per-channel fixture coordinate. It must
        // never be chosen from SQLite's global sequence, even when legacy pins are sparse.
        let channel = post.channel_id.clone();
        let local = st
            .messages
            .keys()
            .filter(|(ch, _)| ch == &channel)
            .map(|(_, seq)| *seq)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Mem local sequence exhausted"))?;
        let ((posted, next), changes) =
            collab::run_with_context(&mut self.store, now, context, |tx| {
                let posted = collab::messages::post(tx, sender, post)?;
                let mut next = st.clone();
                let mut facts = crate::wake_adapter::message_facts(
                    tx.conn(),
                    &posted.message,
                    crewd_wake::Ms(now),
                )?;
                facts.at.seq = local;
                next.messages.insert(
                    (channel.clone(), local),
                    (facts, posted.message.text.clone()),
                );
                let mut messages = existing.clone();
                messages.insert((channel.clone(), local), posted.message.clone());
                let mut inverse = reverse.clone();
                inverse.insert(
                    posted.message.id.clone(),
                    (channel.clone(), local, posted.message.id.clone()),
                );
                for row in collab::wakes::list(tx.conn(), None, 100000)? {
                    let mut actual = super::super::db::queued(tx.conn(), &row)?;
                    let coordinate = actual
                        .msg
                        .as_ref()
                        .and_then(|r| {
                            messages
                                .values()
                                .find(|m| m.channel_id == r.channel && m.seq == r.seq)
                        })
                        .and_then(|m| inverse.get(&m.id))
                        .cloned();
                    if let Some(at) = &mut actual.msg {
                        let (channel, local, _) = coordinate.ok_or_else(|| {
                            crewd_store::StoreError::Invalid(
                                "canonical wake lacks Mem inverse tuple".into(),
                            )
                        })?;
                        at.channel = channel;
                        at.seq = local;
                    }
                    if let Some(old) = next.wakes.iter_mut().find(|old| old.id == actual.id) {
                        *old = actual;
                    } else {
                        next.wakes.push(actual);
                    }
                }
                let mut q = tx
                    .conn()
                    .prepare("SELECT reader,channel_id,seq FROM read_cursors")?;
                for row in q.query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                })? {
                    let (reader, channel, seq) = row?;
                    let local = messages
                        .iter()
                        .filter(|((ch, _), m)| ch == &channel && m.seq <= seq)
                        .max_by_key(|(_, m)| m.seq)
                        .map(|((_, local), _)| *local)
                        .unwrap_or(0);
                    next.cursors.insert((reader, channel), local);
                }
                next.next_wake = next.wakes.iter().map(|w| w.id).max().unwrap_or(0);
                Ok((posted, next))
            })?;
        self.enabled = true;
        self.messages
            .insert((channel.clone(), local), posted.message.clone());
        self.reverse.insert(
            posted.message.id.clone(),
            (channel, local, posted.message.id.clone()),
        );
        Ok((posted.message, next, changes))
    }
    pub fn local_ref(&self, m: &crewd_proto::Message) -> Result<crewd_wake::MessageRef> {
        let (channel, local, _) = self
            .reverse
            .get(&m.id)
            .ok_or_else(|| anyhow::anyhow!("missing actual message inverse"))?;
        let stored = self
            .messages
            .get(&(channel.clone(), *local))
            .ok_or_else(|| anyhow::anyhow!("missing local message tuple"))?;
        anyhow::ensure!(
            stored.id == m.id && stored.seq == m.seq && stored.channel_id == m.channel_id,
            "mismatched actual message identity"
        );
        Ok(crewd_wake::MessageRef {
            channel: channel.clone(),
            seq: *local,
        })
    }
    /// Starts with production fact_of(real committed Change). Only the coordinate is
    /// inverse-projected; no message/fact/policy is manufactured by this adapter.
    pub fn project_fact(
        &self,
        f: super::super::core::CollabFact,
    ) -> Result<super::super::core::CollabFact> {
        if let super::super::core::CollabFact::MessageAppended { channel, seq } = f {
            let m = self
                .messages
                .values()
                .find(|m| m.channel_id == channel && m.seq == seq)
                .ok_or_else(|| anyhow::anyhow!("real MessageAppended lacks canonical inverse"))?;
            let at = self.local_ref(m)?;
            Ok(super::super::core::CollabFact::MessageAppended {
                channel: at.channel,
                seq: at.seq,
            })
        } else {
            Ok(f)
        }
    }
    pub fn progress_memory(
        &mut self,
        st: &MemState,
        session: &str,
        kind: collab::ProgressKind,
        now: i64,
    ) -> Result<(MemState, Vec<collab::Change>)> {
        let context = self.context();
        let ((), changes) = collab::run_with_context(&mut self.store, now, context, |tx| {
            collab::sessions::note_progress(tx, session, kind)
        })?;
        let mut next = st.clone();
        if let Some(row) = next.sessions.get_mut(session) {
            row.last_progress_at = Some(crewd_wake::Ms(now));
        }
        Ok((next, changes))
    }
    pub fn cancel(&mut self, session: &str, now: i64) -> Result<()> {
        let context = self.context();
        collab::run_with_context(&mut self.store, now, context, |tx| {
            collab::wakes::human_stop(tx, session)
        })?;
        Ok(())
    }
    /// Caller runs this inside the last-fallible Tx; projection itself cannot fail after commit.
    pub fn project(tx: &collab::Tx<'_>, st: &MemState) -> collab::Result<MemState> {
        let mut next = st.clone();
        for row in &mut next.wakes {
            let actual = collab::wakes::by_ids(tx.conn(), &[row.id])?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    crewd_store::StoreError::Invalid("missing canonical Mem wake".into())
                })?;
            row.state = match actual.state {
                collab::wakes::WakeState::Pending => crewd_wake::WakeState::Pending,
                collab::wakes::WakeState::Inflight => crewd_wake::WakeState::Inflight,
                collab::wakes::WakeState::Delivered => crewd_wake::WakeState::Delivered,
                collab::wakes::WakeState::Dropped => crewd_wake::WakeState::Dropped,
            };
            row.attempts = actual.attempts;
            row.not_before = crewd_wake::Ms(actual.not_before_ms);
            row.last_error = actual.last_error;
        }
        Ok(next)
    }
    pub fn capture(
        &self,
        session: &str,
        targets: &[(String, i64)],
        now: i64,
    ) -> Result<(cr::Snapshot, causal::CausalSnapshot)> {
        let targets = self.targets(targets)?;
        let channels = targets
            .iter()
            .map(|(channel, _)| channel.clone())
            .collect::<Vec<_>>();
        let snapshot = cr::snapshot(self.store.conn(), session, &channels, now)?;
        let causes = causal::capture(self.store.conn(), &snapshot, &targets)?;
        Ok((snapshot, causes))
    }
    /// Caller must stage/validate ALL Mem row/cursor fault points first. This exact DB Tx is the
    /// last fallible operation; publish the staged Mem rows only after actual commit.
    pub fn claim(
        &mut self,
        prepared: (&cr::Snapshot, &causal::CausalSnapshot),
        targets: &[(String, i64)],
        wake_ids: &[i64],
        rider_ids: &[i64],
        generation: u64,
        now: i64,
    ) -> Result<cr::Decision<causal::ClaimedOutput>> {
        let (snapshot, causes) = prepared;
        let targets = self.targets(targets)?;
        let windows = snapshot.windows_for(&targets)?;
        let context = self.context();
        Ok(
            collab::run_with_context(&mut self.store, now, context, |tx| {
                cr::claim_output(
                    tx,
                    snapshot,
                    causes,
                    &windows,
                    wake_ids,
                    rider_ids,
                    cr::Owner::new(generation)?,
                )
            })?
            .0,
        )
    }
    /// Projection is read INSIDE the same transaction, before COMMIT. No fallible
    /// SQL is performed after an opaque receipt has committed.
    fn receipt<T>(
        &mut self,
        st: &MemState,
        now: i64,
        f: impl FnOnce(&mut collab::Tx<'_>) -> collab::Result<cr::Decision<T>>,
    ) -> Result<(cr::Decision<T>, Option<MemState>)> {
        let context = self.context();
        let messages = self.messages.clone();
        Ok(
            collab::run_with_context(&mut self.store, now, context, |tx| {
                let decision = f(tx)?;
                let next = if matches!(&decision, cr::Decision::Applied(_)) {
                    let mut next = Self::project(tx, st)?;
                    let mut q = tx
                        .conn()
                        .prepare("SELECT reader,channel_id,seq FROM read_cursors")?;
                    for row in q.query_map([], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, i64>(2)?,
                        ))
                    })? {
                        let (reader, channel, seq) = row?;
                        let local = messages
                            .iter()
                            .filter(|((ch, _), m)| ch == &channel && m.seq <= seq)
                            .max_by_key(|(_, m)| m.seq)
                            .map(|((_, local), _)| *local)
                            .unwrap_or(0);
                        next.cursors.insert((reader, channel), local);
                    }
                    Some(next)
                } else {
                    None
                };
                Ok((decision, next))
            })?
            .0,
        )
    }
    pub fn ack_delivered(
        &mut self,
        st: &MemState,
        claimed: &causal::ClaimedOutput,
        success: &causal::ObservedSuccess,
        riders: collab::wakes::Riders<'_>,
        now: i64,
    ) -> Result<(cr::Decision<causal::OutputCommit>, Option<MemState>)> {
        self.receipt(st, now, |tx| {
            cr::ack_output_delivered(tx, claimed, success, riders)
        })
    }
    pub fn ack_failed(
        &mut self,
        st: &MemState,
        claimed: &causal::ClaimedOutput,
        error: &str,
        retry: impl Fn(u32) -> Option<i64>,
        now: i64,
    ) -> Result<(cr::Decision<Vec<collab::wakes::Failed>>, Option<MemState>)> {
        self.receipt(st, now, |tx| {
            cr::ack_output_failed(tx, claimed, error, retry)
        })
    }
    pub fn settle(
        &mut self,
        st: &MemState,
        claimed: &causal::ClaimedOutput,
        fate: cr::Terminal<'_>,
        now: i64,
    ) -> Result<(cr::Decision<()>, Option<MemState>)> {
        self.receipt(st, now, |tx| cr::settle_output(tx, claimed, fate))
    }
    pub fn reset(
        &mut self,
        st: &MemState,
        error: &str,
        retry: impl Fn(u32) -> Option<i64>,
        now: i64,
    ) -> Result<MemState> {
        let context = self.context();
        Ok(
            collab::run_with_context(&mut self.store, now, context, |tx| {
                cr::reset_inflight_and_reservations(tx, error, retry)?;
                Self::project(tx, st)
            })?
            .0,
        )
    }
}
