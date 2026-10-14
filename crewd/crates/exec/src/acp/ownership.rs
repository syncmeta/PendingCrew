//! Core-owned automatic-turn projection. A caller must persist each changed
//! checkpoint in its existing execution authority before releasing an effect.
//! This module creates no file, provider ledger, or process registry writer.
use super::custody::SettledNative;
use crate::registry::ProcessIdentity;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io};

const MAX_BODY: usize = 1_048_576;
const MAX_REQUESTS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeBinding {
    pub session: String,
    pub generation: String,
    pub epoch: u64,
    pub native: ProcessIdentity,
}
impl NativeBinding {
    fn validate(&self) -> io::Result<()> {
        if self.session.is_empty()
            || self.session.len() > 256
            || self.epoch == 0
            || self.native.pid <= 1
            || self.native.pgid != self.native.pid
            || !valid_uuid(&self.generation)
        {
            return Err(denied("invalid native binding"));
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Prepared,
    ContentAccepted,
    Running,
    CancelRequested,
    RawRevoked,
    Unknown,
    Completed,
    Aborted,
    Failed,
}
impl Phase {
    fn settled(self) -> bool {
        matches!(self, Self::Completed | Self::Aborted | Self::Failed)
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    binding: NativeBinding,
    body: String,
    phase: Phase,
    prompt_id: Option<String>,
    turn_id: Option<String>,
}
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request")
            .field("binding", &self.binding)
            .field("body", &"<private>")
            .field("phase", &self.phase)
            .field("prompt_id", &self.prompt_id)
            .field("turn_id", &self.turn_id)
            .finish()
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Checkpoint {
    requests: BTreeMap<String, Request>,
    current: Option<String>,
    manual_binding: Option<NativeBinding>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    Changed,
    Duplicate,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed,
    Aborted,
    Failed,
}
fn denied(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}
fn valid_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}
fn bounded_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
}
/// Runtime creation requires an owning Core persistence callback. The callback
/// must acknowledge durable commit, not merely queue it. Failure releases no
/// prompt/gate/raw-input permission. Checkpoints contain private immutable input;
/// callers must not project them into agent context or diagnostic JSON.
#[derive(Debug, Default)]
pub struct Ownership {
    state: Checkpoint,
}
impl Ownership {
    pub fn checkpoint(&self) -> &Checkpoint {
        &self.state
    }
    pub fn phase(&self, request_id: &str) -> Option<Phase> {
        self.state.requests.get(request_id).map(|r| r.phase)
    }
    fn mutate<F, P>(&mut self, persist: P, change: F) -> io::Result<Applied>
    where
        F: FnOnce(&mut Checkpoint) -> io::Result<Applied>,
        P: FnOnce(&Checkpoint) -> io::Result<()>,
    {
        let mut candidate = self.state.clone();
        let result = change(&mut candidate)?;
        if result == Applied::Changed {
            persist(&candidate)?;
            self.state = candidate;
        }
        Ok(result)
    }
    /// After process/core reconnect, old execution never regains automatic
    /// admission from a saved Prepared/ContentAccepted checkpoint. Its bound
    /// receipt or confirmed native exit is still needed before another prompt.
    pub fn recover<P>(mut checkpoint: Checkpoint, persist: P) -> io::Result<Self>
    where
        P: FnOnce(&Checkpoint) -> io::Result<()>,
    {
        if checkpoint.requests.len() > MAX_REQUESTS {
            return Err(denied("checkpoint limit"));
        }
        if let Some(binding) = &checkpoint.manual_binding {
            binding.validate()?;
        }
        for (id, r) in &checkpoint.requests {
            r.binding.validate()?;
            if !valid_uuid(id)
                || r.body.is_empty()
                || r.body.len() > MAX_BODY
                || r.prompt_id.as_ref().is_some_and(|id| !valid_uuid(id))
                || r.turn_id.as_ref().is_some_and(|id| !bounded_id(id))
                || (matches!(r.phase, Phase::Running | Phase::Completed | Phase::Aborted)
                    && (r.prompt_id.is_none() || r.turn_id.is_none()))
                || (r.phase == Phase::ContentAccepted
                    && (r.prompt_id.is_none() || r.turn_id.is_some()))
                || (r.phase == Phase::Prepared && (r.prompt_id.is_some() || r.turn_id.is_some()))
                || (!r.phase.settled() && checkpoint.current.as_deref() != Some(id))
            {
                return Err(denied("invalid checkpoint"));
            }
        }
        if checkpoint
            .current
            .as_ref()
            .is_some_and(|id| !checkpoint.requests.contains_key(id))
        {
            return Err(denied("missing current request"));
        }
        let mut changed = false;
        for request in checkpoint.requests.values_mut() {
            if !request.phase.settled() {
                request.phase = Phase::Unknown;
                changed = true;
            }
        }
        if changed {
            persist(&checkpoint)?;
        }
        Ok(Self { state: checkpoint })
    }
    /// Same UUID confirms the original request's current state; it never sends
    /// input again. Different payload/identity is a conflict, even after closure.
    pub fn prepare<P>(
        &mut self,
        request_id: &str,
        binding: NativeBinding,
        body: String,
        persist: P,
    ) -> io::Result<Applied>
    where
        P: FnOnce(&Checkpoint) -> io::Result<()>,
    {
        binding.validate()?;
        if !valid_uuid(request_id) || body.is_empty() || body.len() > MAX_BODY {
            return Err(denied("invalid prompt"));
        }
        self.mutate(persist, |state| {
            if let Some(old) = state.requests.get(request_id) {
                return if old.binding == binding && old.body == body {
                    Ok(Applied::Duplicate)
                } else {
                    Err(denied("request payload conflict"))
                };
            }
            if state.manual_binding.is_some() {
                return Err(denied("manual native ownership remains active"));
            }
            if state
                .current
                .as_ref()
                .and_then(|id| state.requests.get(id))
                .is_some_and(|r| !r.phase.settled())
            {
                return Err(denied("previous turn unconfirmed"));
            }
            if state.requests.len() == MAX_REQUESTS {
                return Err(denied("request limit; rotate confirmed native epoch"));
            }
            state.requests.insert(
                request_id.to_owned(),
                Request {
                    binding,
                    body,
                    phase: Phase::Prepared,
                    prompt_id: None,
                    turn_id: None,
                },
            );
            state.current = Some(request_id.to_owned());
            Ok(Applied::Changed)
        })
    }
    fn current_mut<'a>(
        state: &'a mut Checkpoint,
        request_id: &str,
        binding: &NativeBinding,
    ) -> io::Result<&'a mut Request> {
        if state.current.as_deref() != Some(request_id) {
            return Err(denied("stale request"));
        }
        let request = state
            .requests
            .get_mut(request_id)
            .ok_or_else(|| denied("unknown request"))?;
        if &request.binding != binding {
            return Err(denied("foreign native epoch"));
        }
        Ok(request)
    }
    /// Invoke only after actual private bridge authentication and the effective
    /// post-rewrite UserPromptSubmit body is known. Missing prompt UUID, lossy
    /// paste labels, changed bytes and an already running/revoked task all deny.
    pub fn accept_content<P>(
        &mut self,
        request_id: &str,
        binding: &NativeBinding,
        prompt_id: &str,
        effective_body: &str,
        persist: P,
    ) -> io::Result<Applied>
    where
        P: FnOnce(&Checkpoint) -> io::Result<()>,
    {
        if !valid_uuid(prompt_id) {
            return Err(denied("missing real prompt witness"));
        }
        self.mutate(persist, |state| {
            let request = Self::current_mut(state, request_id, binding)?;
            if request.body.as_bytes() != effective_body.as_bytes() {
                return Err(denied("effective content differs"));
            }
            if request.phase == Phase::ContentAccepted
                && request.prompt_id.as_deref() == Some(prompt_id)
            {
                return Ok(Applied::Duplicate);
            }
            if request.phase != Phase::Prepared {
                return Err(denied("automatic ownership revoked or consumed"));
            }
            request.prompt_id = Some(prompt_id.to_owned());
            request.phase = Phase::ContentAccepted;
            Ok(Applied::Changed)
        })
    }
    pub fn turn_started<P>(
        &mut self,
        request_id: &str,
        binding: &NativeBinding,
        prompt_id: &str,
        turn_id: &str,
        persist: P,
    ) -> io::Result<Applied>
    where
        P: FnOnce(&Checkpoint) -> io::Result<()>,
    {
        if !bounded_id(turn_id) {
            return Err(denied("missing turn witness"));
        }
        self.mutate(persist, |state| {
            let request = Self::current_mut(state, request_id, binding)?;
            if request.prompt_id.as_deref() != Some(prompt_id) {
                return Err(denied("foreign prompt witness"));
            }
            if request.phase == Phase::Running && request.turn_id.as_deref() == Some(turn_id) {
                return Ok(Applied::Duplicate);
            }
            if request.turn_id.is_some() {
                return Err(denied("native turn witness already bound"));
            }
            match request.phase {
                Phase::ContentAccepted => request.phase = Phase::Running,
                Phase::CancelRequested | Phase::RawRevoked | Phase::Unknown => {}
                _ => return Err(denied("unaccepted or settled turn")),
            }
            // A late authenticated fact may make cancellation settleable, but
            // cannot change its phase or release manual-input ownership.
            request.turn_id = Some(turn_id.to_owned());
            Ok(Applied::Changed)
        })
    }
    pub fn request_cancel<P>(
        &mut self,
        request_id: &str,
        binding: &NativeBinding,
        persist: P,
    ) -> io::Result<Applied>
    where
        P: FnOnce(&Checkpoint) -> io::Result<()>,
    {
        self.mutate(persist, |state| {
            let request = Self::current_mut(state, request_id, binding)?;
            if request.phase == Phase::CancelRequested {
                return Ok(Applied::Duplicate);
            }
            if request.phase.settled()
                || matches!(request.phase, Phase::RawRevoked | Phase::Unknown)
            {
                return Err(denied("not an automatically owned turn"));
            }
            request.phase = Phase::CancelRequested;
            Ok(Applied::Changed)
        })
    }
    /// Must succeed durably BEFORE forwarding manual PTY bytes. A persistence
    /// failure denies the raw write; successful revocation cannot be undone by
    /// late UserPromptSubmit/turn.start callbacks.
    pub fn revoke_before_raw<P>(
        &mut self,
        binding: &NativeBinding,
        persist: P,
    ) -> io::Result<Applied>
    where
        P: FnOnce(&Checkpoint) -> io::Result<()>,
    {
        binding.validate()?;
        self.mutate(persist, |state| {
            if state
                .manual_binding
                .as_ref()
                .is_some_and(|old| old != binding)
            {
                return Err(denied("foreign manual epoch"));
            }
            let request = state
                .current
                .as_ref()
                .and_then(|id| state.requests.get_mut(id));
            if let Some(request) = request {
                if !request.phase.settled() && &request.binding != binding {
                    return Err(denied("foreign raw input"));
                }
                if !request.phase.settled() {
                    request.phase = Phase::RawRevoked;
                }
            }
            if state.manual_binding.as_ref() == Some(binding) {
                return Ok(Applied::Duplicate);
            }
            state.manual_binding = Some(binding.clone());
            Ok(Applied::Changed)
        })
    }

    /// OS settlement is failure/unknown-turn closure, not successful model
    /// completion. Only Custody can issue this kernel-checked capability.
    pub fn native_exited<P>(&mut self, settled: &SettledNative, persist: P) -> io::Result<Applied>
    where
        P: FnOnce(&Checkpoint) -> io::Result<()>,
    {
        self.mutate(persist, |state| {
            let matches = |binding: &NativeBinding| {
                binding.session == settled.session
                    && binding.generation == settled.generation
                    && binding.epoch == settled.epoch
                    && binding.native == settled.identity
            };
            let mut changed = false;
            if let Some(binding) = &state.manual_binding {
                if !matches(binding) {
                    return Err(denied("foreign manual OS settlement"));
                }
                state.manual_binding = None;
                changed = true;
            }
            if let Some(request) = state
                .current
                .as_ref()
                .and_then(|id| state.requests.get_mut(id))
            {
                if !matches(&request.binding) {
                    return Err(denied("foreign OS settlement"));
                }
                if !request.phase.settled() {
                    request.phase = Phase::Failed;
                    changed = true;
                }
            }
            Ok(if changed {
                Applied::Changed
            } else {
                Applied::Duplicate
            })
        })
    }

    /// Genuine matching turn.complete only. A classic.Stop observation, queued
    /// Escape, transport EOF or silence must NEVER call this method.
    pub fn turn_completed<P>(
        &mut self,
        request_id: &str,
        binding: &NativeBinding,
        prompt_id: &str,
        turn_id: &str,
        outcome: TurnOutcome,
        persist: P,
    ) -> io::Result<Applied>
    where
        P: FnOnce(&Checkpoint) -> io::Result<()>,
    {
        self.mutate(persist, |state| {
            let request = Self::current_mut(state, request_id, binding)?;
            if request.prompt_id.as_deref() != Some(prompt_id)
                || request.turn_id.as_deref() != Some(turn_id)
            {
                return Err(denied("unbound completion"));
            }
            let phase = match outcome {
                TurnOutcome::Completed => Phase::Completed,
                TurnOutcome::Aborted => Phase::Aborted,
                TurnOutcome::Failed => Phase::Failed,
            };
            if request.phase == phase {
                return Ok(Applied::Duplicate);
            }
            if request.phase.settled() {
                return Err(denied("conflicting completion"));
            }
            request.phase = phase;
            Ok(Applied::Changed)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id() -> String {
        uuid::Uuid::new_v4().to_string()
    }
    fn binding() -> NativeBinding {
        NativeBinding {
            session: "real-session".into(),
            generation: id(),
            epoch: 1,
            native: ProcessIdentity {
                pid: 42,
                pgid: 42,
                start_time: 123,
                boot_id: "boot".into(),
                command: "claude".into(),
            },
        }
    }
    fn save(_: &Checkpoint) -> io::Result<()> {
        Ok(())
    }
    #[test]
    fn late_authenticated_start_keeps_cancel_and_manual_ownership_until_matching_complete() {
        for raw in [false, true] {
            let mut o = Ownership::default();
            let b = binding();
            let r = id();
            let p = id();
            o.prepare(&r, b.clone(), "body".into(), save).unwrap();
            o.accept_content(&r, &b, &p, "body", save).unwrap();
            if raw {
                o.revoke_before_raw(&b, save).unwrap();
            } else {
                o.request_cancel(&r, &b, save).unwrap();
            }
            let phase = o.phase(&r);
            let before = o.checkpoint().clone();
            assert!(o.turn_started(&r, &b, &id(), "foreign-turn", save).is_err());
            assert_eq!(o.checkpoint(), &before);
            assert!(
                o.turn_started(&r, &b, &p, "actual-turn", |_| Err(io::Error::other(
                    "real commit denied"
                )))
                .is_err()
            );
            assert_eq!(o.checkpoint(), &before);
            o.turn_started(&r, &b, &p, "actual-turn", save).unwrap();
            assert_eq!(o.phase(&r), phase);
            assert!(o.prepare(&id(), b.clone(), "next".into(), save).is_err());
            o.turn_completed(&r, &b, &p, "actual-turn", TurnOutcome::Aborted, save)
                .unwrap();
            if raw {
                assert!(o.prepare(&id(), b.clone(), "next".into(), save).is_err());
            } else {
                o.prepare(&id(), b.clone(), "next".into(), save).unwrap();
            }
        }
    }
    #[test]
    fn exact_content_is_bound_before_turn_and_duplicate_cannot_reexecute() {
        let mut o = Ownership::default();
        let b = binding();
        let r = id();
        let p = id();
        let body = " a \n世界\n".to_owned();
        o.prepare(&r, b.clone(), body.clone(), save).unwrap();
        let before = o.checkpoint().clone();
        for wrong in [
            "a \n世界\n",
            " a\n世界\n",
            " a \n世界",
            "<pasted_content id=\"1\">...",
        ] {
            assert!(o.accept_content(&r, &b, &p, wrong, save).is_err());
            assert_eq!(o.checkpoint(), &before);
        }
        assert!(o.accept_content(&r, &b, "", &body, save).is_err());
        o.accept_content(&r, &b, &p, &body, save).unwrap();
        assert_eq!(
            o.accept_content(&r, &b, &p, &body, |_| panic!("duplicate persisted"))
                .unwrap(),
            Applied::Duplicate
        );
        o.turn_started(&r, &b, &p, "native-turn", save).unwrap();
        assert!(o.accept_content(&r, &b, &p, &body, save).is_err());
        assert!(
            o.turn_completed(&r, &b, &p, "old-turn", TurnOutcome::Completed, save)
                .is_err()
        );
        o.turn_completed(&r, &b, &p, "native-turn", TurnOutcome::Completed, save)
            .unwrap();
        assert_eq!(
            o.prepare(&r, b.clone(), body, |_| panic!("retry sends again"))
                .unwrap(),
            Applied::Duplicate
        );
        assert!(o.prepare(&r, b, "changed".into(), save).is_err());
        assert!(!format!("{o:?}").contains("世界"));
    }
    #[test]
    fn cancel_is_not_completion_raw_revocation_precedes_bytes_and_recovery_never_regrants() {
        let mut o = Ownership::default();
        let b = binding();
        let r = id();
        let p = id();
        o.prepare(&r, b.clone(), "exact".into(), save).unwrap();
        o.accept_content(&r, &b, &p, "exact", save).unwrap();
        o.turn_started(&r, &b, &p, "turn", save).unwrap();
        o.request_cancel(&r, &b, save).unwrap();
        assert_eq!(o.phase(&r), Some(Phase::CancelRequested));
        assert!(o.prepare(&id(), b.clone(), "next".into(), save).is_err());
        let before = o.checkpoint().clone();
        assert!(
            o.revoke_before_raw(&b, |_| Err(io::Error::other("durability failed")))
                .is_err()
        );
        assert_eq!(o.checkpoint(), &before);
        o.revoke_before_raw(&b, save).unwrap();
        assert!(o.accept_content(&r, &b, &p, "exact", save).is_err());
        assert!(o.turn_started(&r, &b, &p, "turn", save).is_err());
        let mut restored = Ownership::recover(o.checkpoint().clone(), save).unwrap();
        assert_eq!(restored.phase(&r), Some(Phase::Unknown));
        assert!(
            restored
                .prepare(&id(), b.clone(), "next".into(), save)
                .is_err()
        );
        let mut foreign = b.clone();
        foreign.epoch += 1;
        assert!(
            restored
                .turn_completed(&r, &foreign, &p, "turn", TurnOutcome::Aborted, save)
                .is_err()
        );
        restored
            .turn_completed(&r, &b, &p, "turn", TurnOutcome::Aborted, save)
            .unwrap();
        assert!(
            restored
                .prepare(&id(), b.clone(), "next".into(), save)
                .is_err(),
            "old completion cannot release manual native ownership"
        );
        let settled = SettledNative {
            session: b.session.clone(),
            generation: b.generation.clone(),
            epoch: b.epoch,
            identity: b.native.clone(),
        };
        restored.native_exited(&settled, save).unwrap();
        restored.prepare(&id(), b, "next".into(), save).unwrap();
        assert!(
            restored
                .turn_completed(&r, &foreign, &p, "turn", TurnOutcome::Completed, save)
                .is_err()
        );
    }
    #[test]
    fn persistence_failure_cannot_release_content_or_start_and_corrupt_checkpoint_is_rejected() {
        let mut o = Ownership::default();
        let b = binding();
        let r = id();
        let p = id();
        assert!(
            o.prepare(&r, b.clone(), "body".into(), |_| Err(io::Error::other(
                "commit"
            )))
            .is_err()
        );
        assert_eq!(o.checkpoint(), &Checkpoint::default());
        o.prepare(&r, b.clone(), "body".into(), save).unwrap();
        let before = o.checkpoint().clone();
        assert!(
            o.accept_content(&r, &b, &p, "body", |_| Err(io::Error::other("commit")))
                .is_err()
        );
        assert_eq!(o.checkpoint(), &before);
        let mut bad = before;
        bad.current = Some(id());
        assert!(Ownership::recover(bad, save).is_err());
    }
}
