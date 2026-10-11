//! Exact accepted-output fence. It never performs SQL, IO or await under its gate.
use crewd_collab::causal::{
    ObservedSuccess, PendingOutput, PostObservation, PostObservationSource,
};
use crewd_collab::{CollabError, Result};
use crewd_exec::driver::{DeliveryBinding, DeliverySuccessObserver, SendError};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

struct Registration {
    pending: PendingOutput,
    generation: u64,
    delivery: u64,
    published: bool,
}
struct Slot {
    registration: Option<Registration>,
    success: Option<ObservedSuccess>,
    fault: bool,
}
#[derive(Default)]
struct State {
    ordinal: u64,
    slots: BTreeMap<String, Slot>,
}
/// Internal trusted capability; never exposed as a post/RPC flag.
pub struct CausalFence {
    incarnation: String,
    state: Mutex<State>,
}
impl Default for CausalFence {
    fn default() -> Self {
        Self {
            incarnation: uuid::Uuid::new_v4().to_string(),
            state: Mutex::new(State::default()),
        }
    }
}
fn invalid(detail: &str) -> CollabError {
    crewd_store::StoreError::Invalid(format!("causal output fence: {detail}")).into()
}
impl CausalFence {
    pub fn observer(
        self: &Arc<Self>,
        session: &str,
        generation: u64,
    ) -> Arc<dyn DeliverySuccessObserver> {
        Arc::new(Emitter {
            fence: self.clone(),
            session: session.into(),
            generation,
        })
    }
    pub fn binder(
        self: &Arc<Self>,
        pending: &PendingOutput,
        generation: u64,
    ) -> Result<Arc<dyn DeliveryBinding>> {
        if generation == 0 || pending.generation() != generation {
            return Err(invalid("generation mismatch"));
        }
        Ok(Arc::new(Binder {
            fence: self.clone(),
            pending: pending.clone(),
            generation,
        }))
    }
    fn register(&self, pending: &PendingOutput, generation: u64, delivery: u64) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| invalid("poisoned gate"))?;
        let slot = state.slots.entry(pending.session().into()).or_insert(Slot {
            registration: None,
            success: None,
            fault: false,
        });
        if slot.fault {
            return Err(invalid("faulted session fence"));
        }
        if let Some(old) = &slot.registration {
            if old.pending != *pending || old.generation != generation || old.delivery != delivery {
                return Err(invalid("another exact output remains bound"));
            }
            return Ok(());
        }
        slot.registration = Some(Registration {
            pending: pending.clone(),
            generation,
            delivery,
            published: false,
        });
        Ok(())
    }
    pub fn retire(&self, pending: &PendingOutput) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| invalid("poisoned gate"))?;
        if let Some(slot) = state.slots.get_mut(pending.session())
            && slot
                .registration
                .as_ref()
                .is_some_and(|r| r.pending == *pending)
        {
            slot.registration = None;
        }
        Ok(()) // keep immutable last success for post ordering / dedup
    }
    pub fn success(
        &self,
        pending: &PendingOutput,
        generation: u64,
        delivery: u64,
    ) -> Result<ObservedSuccess> {
        let state = self.state.lock().map_err(|_| invalid("poisoned gate"))?;
        let slot = state
            .slots
            .get(pending.session())
            .ok_or_else(|| invalid("missing slot"))?;
        if slot.fault
            || slot.registration.as_ref().is_none_or(|r| {
                r.pending != *pending
                    || r.generation != generation
                    || r.delivery != delivery
                    || !r.published
            })
        {
            return Err(invalid("success does not match exact original output"));
        }
        slot.success
            .clone()
            .ok_or_else(|| invalid("success has not been emitted"))
    }
    fn publish(&self, session: &str, generation: u64, delivery: u64) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| invalid("poisoned gate"))?;
        let Some(slot) = state.slots.get(session) else {
            return Ok(());
        }; // nudge / plain initial brief
        let Some(r) = &slot.registration else {
            return Ok(());
        };
        if r.generation != generation || r.delivery != delivery {
            return Ok(());
        } // old emitter cannot rebind new owner
        if slot.fault {
            return Err(invalid("faulted session fence"));
        }
        let pending = r.pending.clone();
        // A duplicate emitted callback must not refresh or manufacture a second ticket.
        if r.published {
            return Ok(());
        }
        state.ordinal = state
            .ordinal
            .checked_add(1)
            .ok_or_else(|| invalid("ordinal exhausted"))?;
        let success =
            pending.observe_success(delivery, generation, &self.incarnation, state.ordinal)?;
        let slot = state.slots.get_mut(session).expect("checked slot");
        slot.success = Some(success);
        slot.registration
            .as_mut()
            .expect("checked registration")
            .published = true;
        Ok(())
    }
    fn fault(&self, session: &str) {
        if let Ok(mut state) = self.state.lock()
            && let Some(slot) = state.slots.get_mut(session)
        {
            slot.fault = true;
        }
    }
}
/// Persist every new exact episode and its one safe human announcement in this prepare Tx.
pub(crate) fn blocked_channels(
    tx: &mut crewd_collab::Tx<'_>,
    causes: &crewd_collab::causal::CausalSnapshot,
) -> Result<std::collections::BTreeSet<String>> {
    let admission = crewd_collab::causal::inspect_admission(
        tx.conn(),
        causes,
        crewd_collab::causal::CAUSAL_HOP_MAX,
        tx.now_ms(),
    )?;
    let mut blocked = std::collections::BTreeSet::new();
    for channel in admission.channels {
        if channel.blocked {
            blocked.insert(channel.channel);
        }
        for hold in channel.proposed_holds {
            let unresolved = channel.unresolved.iter().any(|u| {
                u.diagnostic_chain_id == hold.chain_id
                    && u.source == hold.from
                    && u.target == hold.target
            });
            if !crewd_collab::causal::hold(tx, &hold)? {
                continue;
            }
            let target = crewd_collab::sessions::get(tx.conn(), &hold.target)?;
            let home = crewd_collab::crews::home_of(tx.conn(), &target.crew_id)?;
            let human = crewd_collab::channels::ensure_human_channel(tx, &home)?;
            let locale = crewd_collab::settings::locale(tx.conn())?;
            let lang = match locale {
                crewd_collab::Locale::Zh => crewd_wake::text::Lang::Zh,
                crewd_collab::Locale::En => crewd_wake::text::Lang::En,
            };
            let target_name = crewd_collab::render::safe_name(
                &crewd_collab::sessions::display_name(tx.conn(), &hold.target)?,
            );
            let origin_label = crewd_collab::sessions::list(tx.conn(), None)?
                .into_iter()
                .find(|s| s.id == hold.from)
                .map(|s| s.display_name.or(s.title).unwrap_or(s.id))
                .unwrap_or_else(|| hold.from.clone());
            let origin_name = crewd_collab::render::safe_name(&origin_label);
            let text = if unresolved {
                crewd_wake::text::render(
                    lang,
                    "loop.chain.unresolved.human",
                    &[
                        ("target", &target_name),
                        ("origin", &origin_name),
                        ("chain", &hold.chain_id),
                    ],
                )
            } else {
                crewd_wake::text::render(
                    lang,
                    "loop.chain.tripped.human",
                    &[
                        ("target", &target_name),
                        ("origin", &origin_name),
                        ("hop", &hold.hop.to_string()),
                        ("chain", &hold.chain_id),
                        ("limit", &crewd_collab::causal::CAUSAL_HOP_MAX.to_string()),
                    ],
                )
            };
            crewd_collab::messages::post(
                tx,
                crewd_collab::Sender::System,
                crewd_collab::messages::NewPost {
                    channel_id: human.id,
                    text,
                    mentions: vec![crewd_proto::Mention {
                        kind: crewd_proto::MentionKind::Human,
                        target_id: None,
                    }],
                    ..Default::default()
                },
            )?;
        }
    }
    Ok(blocked)
}

/// Readonly conversion: candidates never acquire persisted timestamps or ancestry authority.
pub(crate) fn diagnostic_values(
    admission: crewd_collab::causal::Admission,
) -> Vec<(String, crewd_wake::explain::CausalBlockage)> {
    let unknown_channels = admission.unknown_channels;
    let mut values = admission
        .channels
        .into_iter()
        .filter(|c| c.blocked)
        .map(|channel| {
            let blockage = crewd_wake::explain::CausalBlockage {
                cursor_unknown: false,
                holds: channel
                    .holds
                    .into_iter()
                    .map(|h| crewd_wake::explain::CausalChainHold {
                        chain_id: h.chain_id,
                        from: h.from,
                        target: h.target,
                        hop: h.hop,
                        reason: h.reason,
                        at_ms: h.at_ms,
                    })
                    .collect(),
                hold_candidates: channel
                    .proposed_holds
                    .into_iter()
                    .map(|h| crewd_wake::explain::CausalChainCandidate {
                        chain_id: h.chain_id,
                        from: h.from,
                        target: h.target,
                        hop: h.hop,
                        reason: h.reason,
                    })
                    .collect(),
                unresolved: channel
                    .unresolved
                    .into_iter()
                    .map(|u| crewd_wake::explain::CausalUnresolved {
                        message_id: u.message_id,
                        seq: u.seq,
                        from: u.source,
                        target: u.target,
                        output_id: Some(u.parent_receipt),
                    })
                    .collect(),
            };
            (channel.channel, blockage)
        })
        .collect::<Vec<_>>();
    for channel in unknown_channels {
        if let Some((_, blockage)) = values.iter_mut().find(|(ch, _)| *ch == channel) {
            blockage.cursor_unknown = true;
        } else {
            values.push((
                channel,
                crewd_wake::explain::CausalBlockage {
                    cursor_unknown: true,
                    ..Default::default()
                },
            ));
        }
    }
    values
}

impl PostObservationSource for CausalFence {
    fn observe(&self, session: &str) -> Result<PostObservation> {
        let mut state = self.state.lock().map_err(|_| invalid("poisoned gate"))?;
        if state.slots.get(session).is_some_and(|slot| slot.fault) {
            return Err(invalid("faulted session fence"));
        }
        state.ordinal = state
            .ordinal
            .checked_add(1)
            .ok_or_else(|| invalid("ordinal exhausted"))?;
        let candidate = state
            .slots
            .get(session)
            .and_then(|slot| slot.registration.as_ref().map(|r| r.pending.clone()));
        let success = state
            .slots
            .get(session)
            .and_then(|slot| slot.success.clone());
        PostObservation::new(&self.incarnation, state.ordinal, candidate, success)
    }
}
struct Binder {
    fence: Arc<CausalFence>,
    pending: PendingOutput,
    generation: u64,
}
impl DeliveryBinding for Binder {
    fn register(&self, delivery: u64) -> std::result::Result<(), SendError> {
        self.fence
            .register(&self.pending, self.generation, delivery)
            .map_err(|_| SendError::BindingRejected)
    }
    fn cancelled(&self, delivery: u64) {
        // A failed enqueue retires only its own registration, never another generation/ID.
        if let Ok(mut state) = self.fence.state.lock()
            && let Some(slot) = state.slots.get_mut(self.pending.session())
            && slot.registration.as_ref().is_some_and(|r| {
                r.pending == self.pending
                    && r.generation == self.generation
                    && r.delivery == delivery
            })
        {
            slot.registration = None;
        }
    }
}
struct Emitter {
    fence: Arc<CausalFence>,
    session: String,
    generation: u64,
}
impl DeliverySuccessObserver for Emitter {
    fn emitted(&self, delivery: u64) {
        if let Err(error) = self.fence.publish(&self.session, self.generation, delivery) {
            self.fence.fault(&self.session);
            eprintln!("crewd: {error}");
        }
    }
}
