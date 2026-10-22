//! One inert personal-space projection in existing protected Core metadata.
//! Mutations join the original public-command receipt transaction, never execute work.
use crate::{Result, StoreError};
use crewd_proto::{commands::PublicCommand, personal_drafts::*};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
const KEY: &str = "personalDraft.spaces.v1";
const TOTAL_LIMIT: usize = 64 * 1024 * 1024;
fn bad() -> StoreError {
    StoreError::Invalid("personal draft scope, authority, CAS or capacity conflict".into())
}
fn json<T: Serialize>(v: &T) -> Result<String> {
    Ok(serde_json::to_string(v)?)
}
fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn num(s: &str, zero: bool) -> Result<u64> {
    let n = s.parse::<u64>().map_err(|_| bad())?;
    if n.to_string() != s || (!zero && n == 0) || n > i64::MAX as u64 {
        Err(bad())
    } else {
        Ok(n)
    }
}
fn uuid(s: &str) -> Result<()> {
    if crewd_proto::valid_remote_uuid(s) {
        Ok(())
    } else {
        Err(bad())
    }
}
#[derive(Clone)]
pub struct Identity {
    pub principal: String,
    pub local_owner: bool,
    pub local_writer: bool,
    pub remote: Option<crate::notifications::Owner>,
}
impl Identity {
    fn check(&self, c: &Connection) -> Result<()> {
        if let Some(o) = &self.remote {
            o.authorize(c)?;
        } else if !self.local_owner {
            return Err(bad());
        }
        Ok(())
    }
    fn member_matches(&self, m: &Member) -> bool {
        if self.local_writer {
            m.principal == self.principal && m.value.grant_id.is_none()
        } else {
            self.remote.as_ref().is_some_and(|o| {
                m.principal == self.principal
                    && m.value.grant_id.as_deref() == Some(o.grant_id.as_str())
                    && m.value.grant_revision.as_deref() == Some(o.grant_revision.as_str())
                    && m.value.client_spki_sha256.as_deref() == Some(o.client_pin.as_str())
            })
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    principal: String,
    value: PersonalDraftMember,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Branch {
    seq: u64,
    value: PersonalDraftBranch,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Space {
    id: String,
    epoch: String,
    revision: u64,
    next_seq: u64,
    members: Vec<Member>,
    branches: Vec<Branch>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    format: u8,
    current: Option<String>,
    spaces: Vec<Space>,
}
fn load(c: &Connection) -> Result<State> {
    match crate::meta_get(c, KEY)? {
        None => Ok(State {
            format: 1,
            ..Default::default()
        }),
        Some(s) => {
            if s.len() > TOTAL_LIMIT {
                return Err(bad());
            }
            let v: State = serde_json::from_str(&s)?;
            if v.format != 1 || v.spaces.len() > 64 {
                return Err(bad());
            }
            for space in &v.spaces {
                uuid(&space.id)?;
                if space.members.len() > MAX_MEMBERS
                    || space.branches.len() > MAX_BRANCHES
                    || json(space)?.len() > MAX_SPACE_BYTES
                {
                    return Err(bad());
                }
            }
            Ok(v)
        }
    }
}
fn save(c: &Connection, s: &State) -> Result<()> {
    if c.is_autocommit() {
        return Err(bad());
    }
    for space in &s.spaces {
        if json(space)?.len() > MAX_SPACE_BYTES {
            return Err(bad());
        }
    }
    let value = json(s)?;
    if value.len() > TOTAL_LIMIT {
        return Err(bad());
    }
    crate::meta_set(c, KEY, &value)
}
fn current(s: &State) -> Option<&Space> {
    s.current
        .as_ref()
        .and_then(|id| s.spaces.iter().find(|x| &x.id == id))
}
fn current_mut(s: &mut State) -> Result<&mut Space> {
    let id = s.current.clone().ok_or_else(bad)?;
    s.spaces.iter_mut().find(|x| x.id == id).ok_or_else(bad)
}
fn live(c: &Connection, s: &Space) -> Result<()> {
    if s.epoch != crate::judgment_state::epoch(c)? {
        return Err(bad());
    }
    Ok(())
}
fn member<'a>(
    c: &Connection,
    s: &'a Space,
    who: &Identity,
    scope: &PersonalDraftScope,
    require_consent: bool,
) -> Result<&'a Member> {
    who.check(c)?;
    live(c, s)?;
    if s.id != scope.space_id || s.epoch != scope.space_epoch {
        return Err(bad());
    }
    s.members
        .iter()
        .find(|m| {
            m.value.member_id == scope.member_id
                && m.value.revision == scope.member_revision
                && !m.value.revoked
                && who.member_matches(m)
                && (!require_consent || m.value.opted_in)
        })
        .ok_or_else(bad)
}
fn scope_for(s: &Space, m: &Member) -> PersonalDraftScope {
    PersonalDraftScope {
        space_id: s.id.clone(),
        space_epoch: s.epoch.clone(),
        member_id: m.value.member_id.clone(),
        member_revision: m.value.revision.clone(),
    }
}
pub fn key_authorized(c: &Connection, key: &PersonalDraftKey) -> Result<()> {
    let db = crate::meta_get(c, "dbId")?.ok_or_else(bad)?;
    if key.authority_id != db || key.db_id != db {
        return Err(bad());
    }
    let exists: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM crews WHERE id=?1)",
        [&key.crew_id],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(bad());
    }
    let valid: bool = match (&key.destination, key.kind) {
        (
            PersonalDraftDestination::Channel(ch),
            PersonalDraftKind::ChannelMessage | PersonalDraftKind::InitialTask,
        ) => c.query_row(
            "SELECT EXISTS(SELECT 1 FROM channels WHERE id=?1 AND crew_id=?2)",
            rusqlite::params![ch, key.crew_id],
            |r| r.get(0),
        )?,
        (PersonalDraftDestination::OriginalSession(s), PersonalDraftKind::SessionMessage) => c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1 AND crew_id=?2)",
                rusqlite::params![s, key.crew_id],
                |r| r.get(0),
            )?,
        _ => false,
    };
    if !valid {
        return Err(bad());
    }
    Ok(())
}
fn payload(p: &PersonalDraftPayload) -> Result<()> {
    if serde_json::to_vec(p)?.len() > MAX_PAYLOAD_BYTES || p.staged_mentions.len() > 64 {
        return Err(bad());
    }
    for m in p
        .staged_mentions
        .iter()
        .map(|m| &m.mention)
        .chain(p.reply_target.iter().filter_map(|r| r.mention.as_ref()))
    {
        if !matches!(
            m.kind.as_str(),
            "session" | "captain" | "broadcast" | "human"
        ) || m.target_id.as_ref().is_some_and(|v| v.len() > 256)
        {
            return Err(bad());
        }
    }
    Ok(())
}
fn fresh_member(
    principal: String,
    grant: Option<&crewd_proto::RemoteGrantSummary>,
    opted: bool,
) -> Member {
    Member {
        principal,
        value: PersonalDraftMember {
            member_id: id(),
            writer_id: id(),
            revision: "1".into(),
            grant_id: grant.map(|g| g.id.clone()),
            grant_revision: grant.map(|g| g.revision.clone()),
            client_spki_sha256: grant.map(|g| g.client_spki_sha256.clone()),
            opted_in: opted,
            revoked: false,
        },
    }
}
pub fn status(c: &Connection, who: &Identity) -> Result<PersonalDraftStatus> {
    who.check(c)?;
    let state = load(c)?;
    let space = current(&state);
    let is_live =
        space.is_some_and(|s| crate::judgment_state::epoch(c).is_ok_and(|e| e == s.epoch));
    let own = space.and_then(|s| {
        s.members
            .iter()
            .find(|m| who.member_matches(m) && !m.value.revoked)
    });
    let reveal = who.local_owner || own.is_some();
    let m = own.filter(|_| is_live).map(|m| m.value.clone());
    let available = m.as_ref().is_some_and(|m| m.opted_in);
    let reason = if !reveal {
        Some("notApproved")
    } else if space.is_none() {
        Some("notConfigured")
    } else if !is_live {
        Some("restoreRequired")
    } else if own.is_none() {
        Some("notApproved")
    } else if !available {
        Some("optedOut")
    } else {
        None
    };
    Ok(PersonalDraftStatus {
        db_id: crate::meta_get(c, "dbId")?.ok_or_else(bad)?,
        owner: who.local_owner,
        owner_epoch: if who.local_owner {
            Some(crate::judgment_state::epoch(c)?)
        } else {
            None
        },
        available,
        reason: reason.map(str::to_string),
        space_id: space.filter(|_| reveal).map(|s| s.id.clone()),
        space_epoch: space.filter(|_| reveal).map(|s| s.epoch.clone()),
        space_revision: space
            .filter(|_| reveal)
            .map_or("0".into(), |s| s.revision.to_string()),
        member: m,
        payload_limit_bytes: MAX_PAYLOAD_BYTES as u32,
        space_limit_bytes: MAX_SPACE_BYTES as u32,
        branch_limit: MAX_BRANCHES as u32,
        member_limit: MAX_MEMBERS as u32,
    })
}
pub fn members(
    c: &Connection,
    who: &Identity,
    p: &PersonalDraftMembersParams,
) -> Result<PersonalDraftMembers> {
    who.check(c)?;
    if !who.local_owner {
        return Err(bad());
    }
    let state = load(c)?;
    let s = current(&state).ok_or_else(bad)?;
    live(c, s)?;
    if s.id != p.space_id || s.epoch != p.space_epoch {
        return Err(bad());
    }
    Ok(PersonalDraftMembers {
        space_id: s.id.clone(),
        space_epoch: s.epoch.clone(),
        revision: s.revision.to_string(),
        members: s.members.iter().map(|m| m.value.clone()).collect(),
    })
}
fn command_scope(
    command: &PublicCommand,
) -> Option<(&PersonalDraftScope, Option<&PersonalDraftKey>)> {
    match command {
        PublicCommand::PersonalDraftConsent(p) => Some((&p.scope, None)),
        PublicCommand::PersonalDraftCheckpoint(p) => Some((&p.scope, Some(&p.key))),
        PublicCommand::PersonalDraftResolve(p) => Some((&p.scope, Some(&p.key))),
        _ => None,
    }
}
pub fn is_command(command: &PublicCommand) -> bool {
    matches!(
        command,
        PublicCommand::PersonalDraftOwner(_)
            | PublicCommand::PersonalDraftMember(_)
            | PublicCommand::PersonalDraftConsent(_)
            | PublicCommand::PersonalDraftCheckpoint(_)
            | PublicCommand::PersonalDraftResolve(_)
    )
}
/// Used for submission, receipt lookup and the negative fence before any body disclosure.
pub fn authorize_command(
    c: &Connection,
    who: &Identity,
    command: &PublicCommand,
    lookup: bool,
) -> Result<()> {
    who.check(c)?;
    if !is_command(command) {
        return Err(bad());
    }
    if matches!(
        command,
        PublicCommand::PersonalDraftOwner(_) | PublicCommand::PersonalDraftMember(_)
    ) {
        if !who.local_owner {
            return Err(bad());
        }
        match command {
            PublicCommand::PersonalDraftOwner(p) => {
                if p.owner_epoch != crate::judgment_state::epoch(c)? {
                    return Err(bad());
                }
            }
            PublicCommand::PersonalDraftMember(p) => {
                let state = load(c)?;
                let s = current(&state).ok_or_else(bad)?;
                live(c, s)?;
                if s.id != p.space_id || s.epoch != p.space_epoch {
                    return Err(bad());
                }
            }
            _ => return Err(bad()),
        }
        return Ok(());
    }
    let (scope, key) = command_scope(command).ok_or_else(bad)?;
    let state = load(c)?;
    let s = current(&state).ok_or_else(bad)?;
    // A consent's own committed revision can be read after the mutation; this does not authorize another write.
    if lookup {
        live(c, s)?;
        if s.id != scope.space_id
            || s.epoch != scope.space_epoch
            || !s.members.iter().any(|m| {
                m.value.member_id == scope.member_id
                    && !m.value.revoked
                    && who.member_matches(m)
                    && (key.is_none() || m.value.opted_in)
            })
        {
            return Err(bad());
        }
    } else {
        member(
            c,
            s,
            who,
            scope,
            !matches!(command, PublicCommand::PersonalDraftConsent(_)),
        )?;
    }
    if let Some(k) = key {
        key_authorized(c, k)?
    }
    if let PublicCommand::PersonalDraftCheckpoint(p) = command {
        if let Some(b) = s.branches.iter().find(|b| b.value.branch_id == p.branch_id) {
            let m = s
                .members
                .iter()
                .find(|m| {
                    m.value.member_id == scope.member_id
                        && !m.value.revoked
                        && who.member_matches(m)
                })
                .ok_or_else(bad)?;
            if b.value.writer_id != m.value.writer_id || b.value.key != p.key {
                return Err(bad());
            }
        }
    }
    Ok(())
}
pub fn apply(
    c: &Connection,
    who: &Identity,
    command: &PublicCommand,
) -> Result<PersonalDraftMutationResult> {
    if c.is_autocommit() {
        return Err(bad());
    }
    authorize_command(c, who, command, false)?;
    let mut state = load(c)?;
    let mut member_result = None;
    let mut branch_result = None;
    let mut retired = Vec::new();
    match command {
        PublicCommand::PersonalDraftOwner(p) => {
            let before = current(&state);
            if before.map(|s| s.id.as_str()) != p.expected_space_id.as_deref()
                || before.map_or(0, |s| s.revision) != num(&p.expected_space_revision, true)?
            {
                return Err(bad());
            }
            let epoch = crate::judgment_state::epoch(c)?;
            let restored = before.is_some_and(|s| s.epoch != epoch);
            if restored && !p.acknowledge_restore {
                return Err(bad());
            }
            if p.local_enabled && !who.local_writer {
                return Err(bad());
            }
            if before.is_none() || restored {
                if state.spaces.len() >= 64 {
                    return Err(bad());
                }
                let id = id();
                state.current = Some(id.clone());
                state.spaces.push(Space {
                    id,
                    epoch,
                    revision: 0,
                    next_seq: 0,
                    members: vec![],
                    branches: vec![],
                });
            }
            let s = current_mut(&mut state)?;
            if let Some(m) = s
                .members
                .iter_mut()
                .find(|m| m.principal == who.principal && m.value.grant_id.is_none())
            {
                m.value.opted_in = p.local_enabled;
                m.value.revision = (num(&m.value.revision, false)? + 1).to_string();
                member_result = Some(m.value.clone());
            } else if who.local_writer {
                if s.members.len() >= MAX_MEMBERS {
                    return Err(bad());
                }
                let m = fresh_member(who.principal.clone(), None, p.local_enabled);
                member_result = Some(m.value.clone());
                s.members.push(m);
            }
        }
        PublicCommand::PersonalDraftMember(p) => {
            let s = current_mut(&mut state)?;
            live(c, s)?;
            if s.id != p.space_id
                || s.epoch != p.space_epoch
                || s.revision != num(&p.expected_space_revision, true)?
            {
                return Err(bad());
            }
            let identity = crate::remote::identity(c)?.ok_or_else(bad)?;
            let grant =
                crate::remote::grant(c, &p.grant_id, &identity.authority_id)?.ok_or_else(bad)?;
            if grant.revision != p.grant_revision
                || grant.client_spki_sha256 != p.client_spki_sha256
                || (p.enabled && grant.revoked_at.is_some())
            {
                return Err(bad());
            }
            if let Some(m) = s.members.iter_mut().find(|m| {
                m.value.grant_id.as_deref() == Some(p.grant_id.as_str()) && !m.value.revoked
            }) {
                if p.enabled {
                    return Err(bad());
                }
                m.value.revoked = true;
                m.value.opted_in = false;
                m.value.revision = (num(&m.value.revision, false)? + 1).to_string();
                member_result = Some(m.value.clone());
            } else if p.enabled {
                if s.members.len() >= MAX_MEMBERS {
                    return Err(bad());
                }
                let principal = format!("remote:{}:{}", identity.authority_id, grant.id);
                let m = fresh_member(principal, Some(&grant), false);
                member_result = Some(m.value.clone());
                s.members.push(m);
            } else {
                return Err(bad());
            }
        }
        PublicCommand::PersonalDraftConsent(p) => {
            let s = current_mut(&mut state)?;
            let m = s
                .members
                .iter_mut()
                .find(|m| m.value.member_id == p.scope.member_id)
                .ok_or_else(bad)?;
            m.value.opted_in = p.enabled;
            m.value.revision = (num(&m.value.revision, false)? + 1).to_string();
            member_result = Some(m.value.clone());
        }
        PublicCommand::PersonalDraftCheckpoint(p) => {
            uuid(&p.branch_id)?;
            payload(&p.payload)?;
            let s = current_mut(&mut state)?;
            let writer = member(c, s, who, &p.scope, true)?.value.writer_id.clone();
            let next = num(&p.revision, false)?;
            let expected = num(&p.expected_revision, true)?;
            if let Some(b) = s
                .branches
                .iter_mut()
                .find(|b| b.value.branch_id == p.branch_id)
            {
                if b.value.retired
                    || b.value.writer_id != writer
                    || b.value.key != p.key
                    || num(&b.value.revision, false)? != expected
                    || next <= expected
                {
                    return Err(bad());
                }
                b.value.revision = p.revision.clone();
                b.value.payload = Some(p.payload.clone());
                branch_result = Some(b.value.clone());
            } else {
                if expected != 0
                    || s.branches.len() >= MAX_BRANCHES
                    || s.branches.iter().any(|b| {
                        b.value.writer_id == writer && b.value.key == p.key && !b.value.retired
                    })
                {
                    return Err(bad());
                }
                s.next_seq += 1;
                let b = PersonalDraftBranch {
                    branch_id: p.branch_id.clone(),
                    key: p.key.clone(),
                    writer_id: writer,
                    revision: p.revision.clone(),
                    retired: false,
                    payload: Some(p.payload.clone()),
                };
                branch_result = Some(b.clone());
                s.branches.push(Branch {
                    seq: s.next_seq,
                    value: b,
                });
            }
        }
        PublicCommand::PersonalDraftResolve(p) => {
            if p.retiring.is_empty() || p.retiring.len() > 64 {
                return Err(bad());
            }
            let s = current_mut(&mut state)?;
            let writer = member(c, s, who, &p.scope, true)?.value.writer_id.clone();
            let mut ids = std::collections::BTreeSet::new();
            for t in &p.retiring {
                if !ids.insert(t.branch_id.clone()) {
                    return Err(bad());
                }
                let b = s
                    .branches
                    .iter()
                    .find(|b| b.value.branch_id == t.branch_id)
                    .ok_or_else(bad)?;
                if b.value.retired || b.value.key != p.key || b.value.revision != t.revision {
                    return Err(bad());
                }
            }
            if let Some(r) = &p.replacement {
                uuid(&r.branch_id)?;
                num(&r.revision, false)?;
                payload(&r.payload)?;
                if s.branches.len() >= MAX_BRANCHES
                    || s.branches.iter().any(|b| {
                        b.value.branch_id == r.branch_id
                            || (b.value.writer_id == writer
                                && b.value.key == p.key
                                && !b.value.retired
                                && !ids.contains(&b.value.branch_id))
                    })
                {
                    return Err(bad());
                }
            }
            for b in &mut s.branches {
                if ids.contains(&b.value.branch_id) {
                    b.value.retired = true;
                    b.value.payload = None;
                    retired.push(PersonalDraftRetire {
                        branch_id: b.value.branch_id.clone(),
                        revision: b.value.revision.clone(),
                    });
                }
            }
            if let Some(r) = &p.replacement {
                s.next_seq += 1;
                let b = PersonalDraftBranch {
                    branch_id: r.branch_id.clone(),
                    key: p.key.clone(),
                    writer_id: writer,
                    revision: r.revision.clone(),
                    retired: false,
                    payload: Some(r.payload.clone()),
                };
                branch_result = Some(b.clone());
                s.branches.push(Branch {
                    seq: s.next_seq,
                    value: b,
                });
            }
        }
        _ => return Err(bad()),
    }
    let s = current_mut(&mut state)?;
    s.revision = s.revision.checked_add(1).ok_or_else(bad)?;
    let result = PersonalDraftMutationResult {
        operation: command.method().into(),
        space_id: s.id.clone(),
        space_epoch: s.epoch.clone(),
        space_revision: s.revision.to_string(),
        member: member_result,
        branch: branch_result,
        retired,
    };
    save(c, &state)?;
    Ok(result)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    scope: PersonalDraftScope,
    key: Option<PersonalDraftKey>,
    cut: u64,
    last: String,
}
fn cursor(p: &str) -> Result<Cursor> {
    use base64::Engine;
    if p.len() > 4096 {
        return Err(bad());
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(p)
        .map_err(|_| bad())?;
    Ok(serde_json::from_slice(&bytes)?)
}
fn encode(p: &Cursor) -> Result<String> {
    use base64::Engine;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(p)?))
}
fn page(
    scope: &PersonalDraftScope,
    key: Option<&PersonalDraftKey>,
    next: Option<&str>,
    limit: Option<u16>,
    s: &Space,
) -> Result<(usize, u64, Option<String>)> {
    let limit = limit.unwrap_or(32);
    if !(1..=64).contains(&limit) {
        return Err(bad());
    }
    let cut = if let Some(p) = next {
        let p = cursor(p)?;
        if &p.scope != scope || p.key.as_ref() != key || p.cut > s.next_seq {
            return Err(bad());
        }
        (p.cut, Some(p.last))
    } else {
        (s.next_seq, None)
    };
    Ok((limit as usize, cut.0, cut.1))
}
pub fn catalog(
    c: &Connection,
    who: &Identity,
    p: &PersonalDraftCatalogParams,
) -> Result<PersonalDraftCatalogPage> {
    let state = load(c)?;
    let s = current(&state).ok_or_else(bad)?;
    let m = member(c, s, who, &p.scope, true)?;
    let (limit, cut, last) = page(&p.scope, None, p.cursor.as_deref(), p.limit, s)?;
    let mut keys = std::collections::BTreeMap::<String, PersonalDraftKey>::new();
    for b in &s.branches {
        if b.seq <= cut && key_authorized(c, &b.value.key).is_ok() {
            keys.insert(json(&b.value.key)?, b.value.key.clone());
        }
    }
    if last.as_ref().is_some_and(|last| !keys.contains_key(last)) {
        return Err(bad());
    }
    let candidates: Vec<_> = keys
        .into_iter()
        .filter(|(k, _)| last.as_ref().is_none_or(|last| k > last))
        .collect();
    let more = candidates.len() > limit;
    let mut entries = Vec::new();
    let mut end = None;
    for (k, key) in candidates.into_iter().take(limit) {
        let rows: Vec<_> = s
            .branches
            .iter()
            .filter(|b| b.seq <= cut && b.value.key == key)
            .collect();
        entries.push(PersonalDraftCatalogEntry {
            key,
            active_branches: rows.iter().filter(|b| !b.value.retired).count() as u32,
            retired_branches: rows.iter().filter(|b| b.value.retired).count() as u32,
        });
        end = Some(k);
    }
    let scope = scope_for(s, m);
    let next_cursor = if more {
        Some(encode(&Cursor {
            scope: scope.clone(),
            key: None,
            cut,
            last: end.ok_or_else(bad)?,
        })?)
    } else {
        None
    };
    Ok(PersonalDraftCatalogPage {
        scope,
        entries,
        membership_cut: cut.to_string(),
        next_cursor,
        complete: !more,
        observed_at: crate::now_ts(),
    })
}
pub fn branches(
    c: &Connection,
    who: &Identity,
    p: &PersonalDraftBranchesParams,
) -> Result<PersonalDraftBranchPage> {
    let state = load(c)?;
    let s = current(&state).ok_or_else(bad)?;
    let m = member(c, s, who, &p.scope, true)?;
    key_authorized(c, &p.key)?;
    let (limit, cut, last) = page(&p.scope, Some(&p.key), p.cursor.as_deref(), p.limit, s)?;
    let mut rows: Vec<_> = s
        .branches
        .iter()
        .filter(|b| b.seq <= cut && b.value.key == p.key)
        .collect();
    rows.sort_by_key(|b| b.seq);
    let start = if let Some(last) = last {
        rows.iter()
            .position(|b| format!("{}:{}", b.seq, b.value.branch_id) == last)
            .ok_or_else(bad)?
            + 1
    } else {
        0
    };
    let more = rows.len().saturating_sub(start) > limit;
    let values: Vec<_> = rows
        .iter()
        .skip(start)
        .take(limit)
        .map(|b| b.value.clone())
        .collect();
    let scope = scope_for(s, m);
    let next_cursor = if more {
        let b = rows[start + limit - 1];
        Some(encode(&Cursor {
            scope: scope.clone(),
            key: Some(p.key.clone()),
            cut,
            last: format!("{}:{}", b.seq, b.value.branch_id),
        })?)
    } else {
        None
    };
    Ok(PersonalDraftBranchPage {
        scope,
        branches: values,
        membership_cut: cut.to_string(),
        next_cursor,
        complete: !more,
        observed_at: crate::now_ts(),
    })
}
pub fn event_allowed(c: &Connection, who: &Identity, e: &PersonalDraftChanged) -> Result<bool> {
    let state = load(c)?;
    let Some(s) = current(&state) else {
        return Ok(false);
    };
    who.check(c)?;
    live(c, s)?;
    Ok(s.id == e.space_id
        && s.epoch == e.space_epoch
        && s.members
            .iter()
            .any(|m| !m.value.revoked && m.value.opted_in && who.member_matches(m)))
}
/// The exact original request is closed, not applied. All mutations use this same receipt key.
pub fn fence(
    c: &Connection,
    who: &Identity,
    p: &PersonalDraftRequestFenceParams,
) -> Result<PersonalDraftRequestFence> {
    if c.is_autocommit() {
        return Err(bad());
    }
    authorize_command(c, who, &p.original.command, true)?;
    let at = crate::now_ts();
    let (res, state) = match crate::command_receipts::reserve(c, &who.principal, &p.original, &at)?
    {
        crate::command_receipts::Reserved::Existing(r) => {
            let terminal = matches!(
                r.phase,
                crewd_proto::commands::CommandPhase::Committed
                    | crewd_proto::commands::CommandPhase::Rejected
                    | crewd_proto::commands::CommandPhase::Suppressed
            );
            (
                r,
                if terminal {
                    PersonalDraftFenceState::ExistingTerminal
                } else {
                    PersonalDraftFenceState::ExistingPending
                },
            )
        }
        crate::command_receipts::Reserved::New(_) => {
            let r = crate::command_receipts::transition(
                c,
                &who.principal,
                &p.original.db_id,
                &p.original.request_id,
                (
                    crewd_proto::commands::CommandPhase::Reserved,
                    crewd_proto::commands::CommandPhase::Rejected,
                ),
                Some(&serde_json::json!({"reason":"notSubmitted"})),
                &at,
            )?;
            (r, PersonalDraftFenceState::MissingFenced)
        }
    };
    Ok(PersonalDraftRequestFence {
        state,
        receipt: res,
    })
}

#[cfg(test)]
mod tests {
 use super::*;
 fn owner()->Identity{Identity{principal:"local-human:1000".into(),local_owner:true,local_writer:true,remote:None}}
 fn fixture()->crate::Store{let s=crate::Store::open_in_memory().unwrap();s.conn().execute_batch("INSERT INTO crews(id,title,created_at,updated_at)VALUES('draft-crew','synthetic','now','now');INSERT INTO channels(id,crew_id,kind,name,created_by,created_at)VALUES('draft-crew/lobby','draft-crew','lobby','lobby','human','now');").unwrap();s}
 fn configure(s:&mut crate::Store)->PersonalDraftMutationResult{let p=PublicCommand::PersonalDraftOwner(PersonalDraftOwnerParams{owner_epoch:crate::judgment_state::epoch(s.conn()).unwrap(),expected_space_id:None,expected_space_revision:"0".into(),local_enabled:true,acknowledge_restore:false});s.with_tx(|c|apply(c,&owner(),&p)).unwrap()}
 fn scope(s:&crate::Store)->PersonalDraftScope{let status=status(s.conn(),&owner()).unwrap();let m=status.member.unwrap();PersonalDraftScope{space_id:status.space_id.unwrap(),space_epoch:status.space_epoch.unwrap(),member_id:m.member_id,member_revision:m.revision}}
 fn key(s:&crate::Store)->PersonalDraftKey{let db=crate::meta_get(s.conn(),"dbId").unwrap().unwrap();PersonalDraftKey{authority_id:db.clone(),db_id:db,crew_id:"draft-crew".into(),destination:PersonalDraftDestination::Channel("draft-crew/lobby".into()),kind:PersonalDraftKind::ChannelMessage}}
 fn body(text:&str)->PersonalDraftPayload{PersonalDraftPayload{text:text.into(),staged_mentions:vec![PersonalDraftStagedMention{token:"@synthetic".into(),mention:PersonalDraftMention{kind:"human".into(),target_id:None}}],reply_target:Some(PersonalDraftReply{mention:None,reply_to_id:"synthetic-message".into(),quoted_sender:"synthetic".into(),quoted_snippet:"exact 原文".into()})}}
 fn checkpoint(s:&crate::Store,branch:&str,expected:&str,revision:&str,text:&str)->PublicCommand{PublicCommand::PersonalDraftCheckpoint(PersonalDraftCheckpointParams{scope:scope(s),branch_id:branch.into(),key:key(s),expected_revision:expected.into(),revision:revision.into(),payload:body(text)})}
 #[test]
 fn core_draft_normal_remote_configuration_does_not_rotate_personal_namespace(){let mut s=fixture();let _=configure(&mut s);let before=status(s.conn(),&owner()).unwrap();assert!(before.available);s.with_tx(|c|{assert!(crate::remote::configure(c,1,false,"127.0.0.1",7423)?);Ok(())}).unwrap();let after=status(s.conn(),&owner()).unwrap();assert!(after.available,"ordinary remote CAS must not become restoreRequired: {after:?}");assert_eq!(after.space_epoch,before.space_epoch);assert_eq!(after.member.unwrap().writer_id,before.member.unwrap().writer_id);}
 #[test]
 fn core_draft_exact_payload_cas_send_clear_and_permanent_tombstone(){let mut s=fixture();configure(&mut s);let branch=id();let p=checkpoint(&s,&branch,"0","2","exact\n原文");s.with_tx(|c|apply(c,&owner(),&p)).unwrap();let stale=checkpoint(&s,&branch,"0","3","must not replace");assert!(s.with_tx(|c|apply(c,&owner(),&stale)).is_err());let clear=checkpoint(&s,&branch,"2","4","");s.with_tx(|c|apply(c,&owner(),&clear)).unwrap();let page=branches(s.conn(),&owner(),&PersonalDraftBranchesParams{scope:scope(&s),key:key(&s),cursor:None,limit:None}).unwrap();assert_eq!(page.branches[0].payload,Some(body("")));let fresh=id();let r=PublicCommand::PersonalDraftResolve(PersonalDraftResolveParams{scope:scope(&s),key:key(&s),retiring:vec![PersonalDraftRetire{branch_id:branch.clone(),revision:"4".into()}],replacement:Some(PersonalDraftReplacement{branch_id:fresh,revision:"1".into(),payload:body("explicit merged original")})});let result=s.with_tx(|c|apply(c,&owner(),&r)).unwrap();assert_eq!(result.retired[0].revision,"4");let late=checkpoint(&s,&branch,"4","999","late offline 原文");assert!(s.with_tx(|c|apply(c,&owner(),&late)).is_err());let page=branches(s.conn(),&owner(),&PersonalDraftBranchesParams{scope:scope(&s),key:key(&s),cursor:None,limit:Some(1)}).unwrap();assert!(!page.complete);assert!(page.branches[0].retired);assert!(page.branches[0].payload.is_none());}
 #[test]
 fn core_draft_negative_fence_uses_original_receipt_and_never_executes_late_packet(){use crate::command_receipts::{self as receipts,Reserved};let mut s=fixture();configure(&mut s);let command=checkpoint(&s,&id(),"0","1","must stay only local");let original=crewd_proto::commands::CommandSubmitParams{request_id:id(),db_id:crate::meta_get(s.conn(),"dbId").unwrap().unwrap(),runtime_binding_id:id(),command:command.clone()};let result=s.with_tx(|c|fence(c,&owner(),&PersonalDraftRequestFenceParams{original:original.clone()})).unwrap();assert_eq!(result.state,PersonalDraftFenceState::MissingFenced);assert_eq!(result.receipt.phase,crewd_proto::commands::CommandPhase::Rejected);s.with_tx(|c|{let r=receipts::reserve(c,&owner().principal,&original,"late")?;assert!(matches!(r,Reserved::Existing(_)));Ok(())}).unwrap();assert!(load(s.conn()).unwrap().spaces[0].branches.is_empty());let mut changed=original.clone();if let PublicCommand::PersonalDraftCheckpoint(p)=&mut changed.command{p.payload.text="different".into()}assert!(s.with_tx(|c|fence(c,&owner(),&PersonalDraftRequestFenceParams{original:changed})).is_err());let again=s.with_tx(|c|fence(c,&owner(),&PersonalDraftRequestFenceParams{original})).unwrap();assert_eq!(again.state,PersonalDraftFenceState::ExistingTerminal);}
}
