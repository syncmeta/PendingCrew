//! Shared read-only message/context adaptation for orchestration and post predictions.
use crewd_collab as collab;
use crewd_proto::{ChannelKind, MemberRef, MentionKind, Message, SenderKind};
use crewd_wake::plan::{ChannelCtx, Listen, Mention, MsgFacts, Sender};
use crewd_wake::{MessageRef, Ms};

fn ms_of(ts: &str) -> Option<Ms> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|t| Ms(t.timestamp_millis()))
}
fn opt<T>(r: collab::Result<T>) -> collab::Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(collab::CollabError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}
/// 消息 → 规划要素。`now` 只在读不出发送时刻时用：读不出 = 当作刚发的（绝不当成陈旧而不叫，ledger NOTE T2）。
pub(crate) fn facts_of(m: &Message, now: Ms) -> MsgFacts {
    let sender = match (m.sender_kind, &m.sender_session_id) {
        (SenderKind::Human, _) => Sender::Human,
        (SenderKind::Session | SenderKind::Captain, Some(id)) => Sender::Session {
            id: id.clone(),
            captain: m.sender_kind == SenderKind::Captain,
        },
        _ => Sender::System,
    };
    // @ 只决定叫醒谁（§24.2）。没有目标的 session / 机长 @（旧数据：机组没有机长时没落定的 @机长）叫不醒任何人，略过；
    // 人发的消息因此没指向任何成员时照兜底规则走（Option A）。
    let mentions = m
        .mentions
        .iter()
        .filter_map(|x| match (x.kind, &x.target_id) {
            (MentionKind::Session | MentionKind::Captain, Some(t)) => {
                Some(Mention::Session(t.clone()))
            }
            (MentionKind::Session | MentionKind::Captain, None) => None,
            (MentionKind::Human, _) => Some(Mention::Human),
        })
        .collect();
    let created_at = ms_of(&m.created_at).unwrap_or_else(|| {
        eprintln!(
            "crewd: 编排：消息 {} 的发送时刻读不出（{:?}），按刚发的算",
            m.id, m.created_at
        );
        now
    });
    MsgFacts {
        provenance: crewd_wake::plan::MessageProvenance::Ordinary,
        id: m.id.clone(),
        at: MessageRef {
            channel: m.channel_id.clone(),
            seq: m.seq,
        },
        sender,
        mentions,
        created_at,
    }
}

/// Keep provenance read/validation in the same DB operation as the message facts.
pub(crate) fn message_facts(
    conn: &rusqlite::Connection,
    message: &Message,
    now: Ms,
) -> collab::Result<MsgFacts> {
    let mut facts = facts_of(message, now);
    facts.provenance = match collab::message_provenance::read(conn, message)? {
        collab::message_provenance::MessageProvenance::Ordinary => {
            crewd_wake::plan::MessageProvenance::Ordinary
        }
        collab::message_provenance::MessageProvenance::StopRelay { .. } => {
            crewd_wake::plan::MessageProvenance::StopRelay
        }
    };
    Ok(facts)
}

fn session_ids(ms: Vec<MemberRef>) -> Vec<String> {
    ms.into_iter()
        .filter_map(|m| match m {
            MemberRef::Session { id } => Some(id),
            MemberRef::Human => None,
        })
        .collect()
}

pub(crate) fn channel_context(
    c: &rusqlite::Connection,
    channel: &str,
    now: Ms,
) -> collab::Result<Option<ChannelCtx>> {
    let Some(ch) = opt(collab::channels::get(c, channel))? else {
        return Ok(None);
    };
    // 成员 = M3a 的有效成员（`channels::members`），且与注入面（`cursors::unread_for` 用的
    // `channels_of_session`）同一口径：归档的 topic / dm / 人类群里没有人「在」（大厅照算：它的机组
    // 被移除时大厅归档，大厅仍在它的 session 的注入面上）。
    let archived_out = ch.archived_at.is_some() && ch.kind != ChannelKind::Lobby;
    let session_members = if archived_out {
        Vec::new()
    } else {
        session_ids(collab::channels::members(c, channel)?)
    };
    let owner = match ch.owner {
        Some(MemberRef::Session { id }) => Some(id),
        _ => None,
    };
    let crew_captain = collab::crews::captain_of(c, &ch.crew_id)?;
    let dm_sessions = if ch.kind == ChannelKind::Dm {
        session_members.clone()
    } else {
        Vec::new()
    };
    // 传真实的 now（不是 0）：0 会把已过期的收听也读回来。归档的群、读不到的收听 collab 已滤掉。
    let active = collab::listens::active_for_channel(c, channel, now.0)?;
    // 活着 = 存在且机组没被移除（不是「进程在跑」）：群成员 ∪ 机组没被移除的收听者。
    let mut live_sessions = session_members.clone();
    for l in &active {
        let Some(s) = crewd_store::sessions::get(c, &l.session)? else {
            continue;
        };
        if !collab::crews::is_archived(c, &s.crew_id)? && !live_sessions.contains(&s.id) {
            live_sessions.push(s.id);
        }
    }
    let listens = active
        .into_iter()
        .map(|l| Listen {
            session: l.session,
            until: l.until_ms.map_or(Ms(i64::MAX), Ms),
            senders: (!l.senders.is_empty()).then_some(l.senders),
        })
        .collect();
    Ok(Some(ChannelCtx {
        session_members,
        live_sessions,
        owner,
        dm_sessions,
        crew_captain,
        listens,
    }))
}

// Post receipt prediction uses the same readonly facts/context above.
use crate::AppState;
use crate::agent::port::{PortError, PortResult};
use crewd_collab::Locale;
use rusqlite::Connection;

/// Prospective planner targets, not a delivery receipt. Deduplication is by identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PredictedWake {
    pub session: String,
    pub label: String,
}

#[derive(Clone)]
struct CommittedRef {
    id: String,
    channel: String,
    seq: i64,
}

#[cfg(test)]
pub(crate) type PredictionTestControls = std::sync::Arc<
    std::sync::Mutex<std::collections::BTreeMap<String, std::sync::Arc<PredictionTestControl>>>,
>;

/// Synthetic postcommit read-error boundary; it does not simulate disk failure.
#[cfg(test)]
pub(crate) struct PredictionTestControl {
    pub now: Option<crewd_wake::Ms>,
    pub fail_read: bool,
}

#[cfg(test)]
pub(crate) struct PredictionTestGuard {
    controls: PredictionTestControls,
    poster: String,
    registration: std::sync::Arc<PredictionTestControl>,
}

#[cfg(test)]
impl Drop for PredictionTestGuard {
    fn drop(&mut self) {
        let mut controls = self.controls.lock().unwrap();
        if controls
            .get(&self.poster)
            .is_some_and(|c| std::sync::Arc::ptr_eq(c, &self.registration))
        {
            controls.remove(&self.poster);
        }
    }
}

#[cfg(test)]
pub(crate) fn set_prediction_test_control(
    st: &AppState,
    poster: &str,
    control: PredictionTestControl,
) -> PredictionTestGuard {
    let registration = std::sync::Arc::new(control);
    let mut controls = st.s72_prediction_tests.lock().unwrap();
    assert!(
        !controls.contains_key(poster),
        "prediction control is already armed"
    );
    controls.insert(poster.to_string(), registration.clone());
    PredictionTestGuard {
        controls: st.s72_prediction_tests.clone(),
        poster: poster.to_string(),
        registration,
    }
}

// Private typed owner boundary. Existing Port strings/refusals are not DB evidence.
#[derive(Debug)]
enum PredictionError {
    Collab(collab::CollabError),
    Store(crewd_store::StoreError),
    Port(PortError),
}
impl From<collab::CollabError> for PredictionError {
    fn from(error: collab::CollabError) -> Self {
        Self::Collab(error)
    }
}
impl From<crewd_store::StoreError> for PredictionError {
    fn from(error: crewd_store::StoreError) -> Self {
        Self::Store(error)
    }
}
impl From<PortError> for PredictionError {
    fn from(error: PortError) -> Self {
        Self::Port(error)
    }
}
impl crewd_store::db_health::DbFaultSource for PredictionError {
    fn db_fault_kind(&self) -> Option<crewd_store::db_health::DbFaultKind> {
        match self {
            Self::Collab(error) => crewd_store::db_health::DbFaultSource::db_fault_kind(error),
            Self::Store(error) => crewd_store::db_health::DbFaultSource::db_fault_kind(error),
            Self::Port(_) => None,
        }
    }
}
impl PredictionError {
    fn into_port(self) -> PortError {
        match self {
            Self::Collab(error) => error.into(),
            Self::Store(error) => error.into(),
            Self::Port(error) => error,
        }
    }
}
type PredictionResult<T> = Result<T, PredictionError>;

/// One read-only DB job, one time snapshot, actual committed identities only.
pub async fn predict_wakes(
    st: &AppState,
    poster: &str,
    posted: &[Message],
    loc: Locale,
) -> PortResult<Vec<PredictedWake>> {
    if posted.is_empty() {
        return Ok(vec![]);
    }
    let refs: Vec<_> = posted
        .iter()
        .map(|m| CommittedRef {
            id: m.id.clone(),
            channel: m.channel_id.clone(),
            seq: m.seq,
        })
        .collect();
    let poster = poster.to_string();
    #[cfg(test)]
    let controls = st.s72_prediction_tests.clone();
    st.db
        .call_result(move |s| {
            #[cfg(test)]
            let control = { controls.lock().unwrap().remove(&poster) };
            let now = crewd_wake::Ms(crewd_store::now_ms());
            #[cfg(test)]
            let now = control.as_ref().and_then(|c| c.now).unwrap_or(now);
            let messages = committed_messages(s.conn(), &poster, &refs)?;
            #[cfg(test)]
            if control.as_ref().is_some_and(|c| c.fail_read) {
                return Err(PortError::Internal("synthetic prediction read failure".into()).into());
            }
            predict_messages(s.conn(), &messages, loc, now)
        })
        .await?
        .map_err(PredictionError::into_port)
}

fn committed_messages(
    conn: &Connection,
    poster: &str,
    refs: &[CommittedRef],
) -> PredictionResult<Vec<Message>> {
    let mut out = Vec::with_capacity(refs.len());
    for r in refs {
        if r.id.is_empty() || r.channel.is_empty() || r.seq <= 0 {
            return Err(PortError::Internal("invalid committed message identity".into()).into());
        }
        let m = crewd_collab::messages::get(conn, &r.id)?;
        if m.id != r.id
            || m.channel_id != r.channel
            || m.seq != r.seq
            || !matches!(
                m.sender_kind,
                crewd_proto::SenderKind::Session | crewd_proto::SenderKind::Captain
            )
            || m.sender_session_id.as_deref() != Some(poster)
        {
            return Err(PortError::Internal("committed message identity mismatch".into()).into());
        }
        out.push(m);
    }
    Ok(out)
}

fn predict_messages(
    conn: &Connection,
    messages: &[Message],
    loc: Locale,
    now: crewd_wake::Ms,
) -> PredictionResult<Vec<PredictedWake>> {
    let mut out: Vec<PredictedWake> = Vec::new();
    for m in messages {
        let facts = crate::wake_adapter::message_facts(conn, m, now)?;
        let ctx = crate::wake_adapter::channel_context(conn, &m.channel_id, now)?
            .ok_or_else(|| PortError::Internal("committed message channel is missing".into()))?;
        for target in crewd_wake::plan::plan(&facts, &ctx, None, now) {
            if out.iter().any(|p| p.session == target.session) {
                continue;
            }
            if crewd_store::sessions::get(conn, &target.session)?.is_none() {
                return Err(PortError::Internal("planned target session is missing".into()).into());
            }
            let label = crewd_collab::render::target_label(conn, loc, &target.session)?;
            out.push(PredictedWake {
                session: target.session,
                label,
            });
        }
    }
    Ok(out)
}
