//! Opt-in, pure typed differences between actual `BookState` values.
//!
//! This is data, not a persistence/Actor/Scope capability. No event is replayed to
//! reconstruct a value. A tracked delta owns one complete typed baseline so even
//! an unchanged part of the wrong baseline is rejected. Changed entities retain
//! their complete original values, including nested fields and collection order.
//! This deliberately costs a full baseline plus changed-value clones when opted
//! in; ordinary `WorkBook::new`/`restore` do not allocate or clone for tracking.
//! A future save must pass its real canonical before/after states to `validate`
//! in addition to the original domain, transaction and authorization checks.
use crate::ids::{AskId, CrewId, EntryNo, SessionId, Ts, WorkId};
use crate::model::{BookState, EntryTodo, HumanAsk, Organization, StopStreak, WorkItem};
use std::collections::BTreeMap;

/// Exact first and last values. Map changes use `Option<T>` for absence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change<T> {
    pub before: T,
    pub after: T,
}

/// All persisted non-entity metadata, including full organization attribution.
/// `None` organization and `Some(empty)` remain different values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookMetadata {
    pub crew: CrewId,
    pub organization: Option<Organization>,
    pub max_parent_depth: Option<u8>,
    pub last_item: u32,
    pub last_ask: u32,
    pub last_entry: i64,
    pub last_seq: u64,
    pub clock: Ts,
}

impl BookMetadata {
    fn from_state(state: &BookState) -> Self {
        // Deliberately exhaustive: a new BookState field must be classified,
        // rather than silently disappearing behind `..` or a serde projection.
        let BookState {
            crew,
            organization,
            max_parent_depth,
            items: _,
            entries: _,
            asks: _,
            last_item,
            last_ask,
            last_entry,
            last_seq,
            clock,
            stop_streaks: _,
        } = state;
        Self {
            crew: crew.clone(),
            organization: organization.clone(),
            max_parent_depth: *max_parent_depth,
            last_item: *last_item,
            last_ask: *last_ask,
            last_entry: *last_entry,
            last_seq: *last_seq,
            clock: *clock,
        }
    }
}

/// Complete changed values, never a partial WorkItem or an EventKind-derived view.
/// An unchanged/no-op entry is not canonical and fails exact validation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BookChanges {
    pub metadata: Option<Change<BookMetadata>>,
    pub items: BTreeMap<WorkId, Change<Option<WorkItem>>>,
    pub entries: BTreeMap<EntryNo, Change<Option<EntryTodo>>>,
    pub asks: BTreeMap<AskId, Change<Option<HumanAsk>>>,
    pub stop_streaks: BTreeMap<SessionId, Change<Option<StopStreak>>>,
}

impl BookChanges {
    fn between(before: &BookState, after: &BookState) -> Self {
        let before_metadata = BookMetadata::from_state(before);
        let after_metadata = BookMetadata::from_state(after);
        Self {
            metadata: (before_metadata != after_metadata).then_some(Change {
                before: before_metadata,
                after: after_metadata,
            }),
            items: map_changes(&before.items, &after.items),
            entries: map_changes(&before.entries, &after.entries),
            asks: map_changes(&before.asks, &after.asks),
            stop_streaks: map_changes(&before.stop_streaks, &after.stop_streaks),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.metadata.is_none()
            && self.items.is_empty()
            && self.entries.is_empty()
            && self.asks.is_empty()
            && self.stop_streaks.is_empty()
    }
}

fn map_changes<K: Ord + Clone, V: PartialEq + Clone>(
    before: &BTreeMap<K, V>,
    after: &BTreeMap<K, V>,
) -> BTreeMap<K, Change<Option<V>>> {
    let mut changes = BTreeMap::new();
    for (key, old) in before {
        let new = after.get(key);
        if new != Some(old) {
            changes.insert(
                key.clone(),
                Change {
                    before: Some(old.clone()),
                    after: new.cloned(),
                },
            );
        }
    }
    for (key, new) in after {
        if !before.contains_key(key) {
            changes.insert(
                key.clone(),
                Change {
                    before: None,
                    after: Some(new.clone()),
                },
            );
        }
    }
    changes
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaError {
    BaselineMismatch,
    ChangesMismatch,
}

impl std::fmt::Display for DeltaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::BaselineMismatch => "persistent delta does not match the actual baseline",
            Self::ChangesMismatch => "persistent delta is not the complete actual state difference",
        })
    }
}

impl std::error::Error for DeltaError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistentDelta {
    pub before: BookState,
    pub changes: BookChanges,
}

impl PersistentDelta {
    /// Compare whole actual typed values. This neither changes a BookState nor
    /// asserts that a caller-provided image has domain/DB authority.
    pub fn between(before: &BookState, after: &BookState) -> Self {
        Self {
            before: before.clone(),
            changes: BookChanges::between(before, after),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Reject a different full baseline, missing/extra/no-op changes, stale
    /// before values, wrong attribution and any omitted metadata/entity field.
    /// The two arguments must be the caller's real canonical states; this data
    /// object never replaces the original invariants, permissions or receipt.
    pub fn validate(&self, before: &BookState, after: &BookState) -> Result<(), DeltaError> {
        if self.before != *before {
            return Err(DeltaError::BaselineMismatch);
        }
        if self.changes != BookChanges::between(before, after) {
            return Err(DeltaError::ChangesMismatch);
        }
        Ok(())
    }

    /// Compose only across the real shared intermediate image. Overlap checks
    /// alone cannot detect a different untouched entity or organization map.
    /// Net-zero entity changes disappear, while real clock/floor/sequence/streak
    /// changes remain. Callers still validate the result at the actual save.
    pub fn compose(
        &self,
        next: &Self,
        before: &BookState,
        middle: &BookState,
        after: &BookState,
    ) -> Result<Self, DeltaError> {
        self.validate(before, middle)?;
        next.validate(middle, after)?;
        Ok(Self::between(before, after))
    }

    /// WorkBook owns the actual state replacement; keep its original anchor
    /// rather than allocating another whole baseline after each command.
    pub(crate) fn refresh_after(&mut self, after: &BookState) {
        self.changes = BookChanges::between(&self.before, after);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Actor, AskState, Blocker, Body, ChannelId, Config, DefaultOption, DoneLevel, DropKind,
        DropReason, Evidence, HowChanged, NewItem, Note, OwnerChange, PendingTransfer, Progress,
        Rejected, Report, Status, StopDecision, StopKind, StopViolation, WaitTier, WorkBook,
    };

    fn book() -> WorkBook {
        WorkBook::new(CrewId::new("crew-a"), Config::default())
    }

    fn create_owned(book: &mut WorkBook) -> WorkId {
        book.create_item(
            &Actor::Human,
            Ts(1),
            NewItem::titled("original").owned_by("worker"),
        )
        .unwrap()
        .value
    }

    #[test]
    fn same_events_different_goal_acceptance_survive() {
        let mut a = book();
        let mut b = book();
        let before = a.state().clone();
        a.enable_persistent_delta_tracking();
        b.enable_persistent_delta_tracking();
        let item = |goal: &str, acceptance: &str| NewItem {
            body: Body {
                goal: goal.into(),
                acceptance: acceptance.into(),
                level: DoneLevel::Production,
            },
            ..NewItem::titled("same title")
        };
        let left = a
            .create_item(&Actor::Human, Ts(7), item("goal A", "acceptance A"))
            .unwrap();
        let right = b
            .create_item(&Actor::Human, Ts(7), item("goal B", "acceptance B"))
            .unwrap();
        assert_eq!(left.events, right.events);
        assert_ne!(a.state(), b.state());
        let da = a.take_persistent_delta().unwrap();
        let db = b.take_persistent_delta().unwrap();
        da.validate(&before, a.state()).unwrap();
        db.validate(&before, b.state()).unwrap();
        assert_ne!(da, db);
        let changed = da.changes.items[&left.value].after.as_ref().unwrap();
        assert_eq!(changed.body.goal, "goal A");
        assert_eq!(changed.body.acceptance, "acceptance A");
        assert_eq!(changed.body.level, DoneLevel::Production);
    }

    #[test]
    fn all_work_fields_options_and_order_survive() {
        let mut source = book();
        let id = create_owned(&mut source);
        let before = source.state().clone();
        // Pure typed-image coverage, not a claim that mutually exclusive
        // lifecycle options would be accepted together by a WorkBook command.
        // Domain admission remains in the original invariants/command layer.
        let expected = WorkItem {
            id,
            title: "full title".into(),
            body: Body {
                goal: "目标\0完整".repeat(20_000),
                level: DoneLevel::Public,
                acceptance: "acceptance\nwith Unicode 台湾".repeat(10_000),
            },
            parent: Some(WorkId(9)),
            depends_on: vec![WorkId(8), WorkId(2)],
            sources: vec![EntryNo(7), EntryNo(3)],
            owner: Some(SessionId::new("next")),
            owner_history: vec![
                before.items[&id].owner_history[0].clone(),
                OwnerChange {
                    from: Some(SessionId::new("worker")),
                    to: SessionId::new("next"),
                    by: Actor::Human,
                    how: HowChanged::Transfer,
                    note: "whole transfer history".into(),
                    at: Ts(4),
                },
            ],
            status: Status::Blocked,
            blocked_on: vec![Blocker::Item(WorkId(8)), Blocker::Human(AskId(4))],
            next_check_at: Some(Ts(100)),
            missed_checks: 9,
            evidence: Some(Evidence::Commit("full commit".into())),
            completion_note: Some("completion".into()),
            drop_reason: Some(DropReason {
                kind: DropKind::Superseded,
                note: "whole reason".into(),
                evidence: Some(Evidence::Text("full reason evidence".into())),
            }),
            pending_transfer: Some(PendingTransfer {
                to: SessionId::new("third"),
                requested_by: SessionId::new("next"),
                note: "pending note".into(),
                at: Ts(8),
            }),
            channel: Some(ChannelId::new("channel")),
            last_report: Some(Note {
                text: "the complete report".into(),
                at: Ts(9),
            }),
            created_by: Some(SessionId::new("creator")),
            revision: u64::MAX,
            created_at: Ts(-50),
            updated_at: Ts(10),
        };
        let mut after = before.clone();
        after.items.insert(id, expected.clone());
        let delta = PersistentDelta::between(&before, &after);
        delta.validate(&before, &after).unwrap();
        let change = &delta.changes.items[&id];
        assert_eq!(change.before.as_ref(), before.items.get(&id));
        assert_eq!(change.after.as_ref(), Some(&expected));
        assert_eq!(
            change.after.as_ref().unwrap().depends_on,
            [WorkId(8), WorkId(2)]
        );
        assert_eq!(
            change.after.as_ref().unwrap().sources,
            [EntryNo(7), EntryNo(3)]
        );
        assert_eq!(
            serde_json::to_vec(change.after.as_ref().unwrap()).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
        let mut omitted = delta.clone();
        omitted
            .changes
            .items
            .get_mut(&id)
            .unwrap()
            .after
            .as_mut()
            .unwrap()
            .body
            .goal
            .clear();
        assert_eq!(
            omitted.validate(&before, &after),
            Err(DeltaError::ChangesMismatch)
        );
    }

    #[test]
    fn complete_ask_entry_stop_and_organization_roundtrip() {
        let mut b = WorkBook::organization(Config::default());
        b.set_default_crew(CrewId::new("crew-a"));
        let before = b.state().clone();
        b.enable_persistent_delta_tracking();
        b.register_entry_in(Ts(1), EntryNo(5), CrewId::new("crew-a"))
            .unwrap();
        let id = b
            .create_item(
                &Actor::Human,
                Ts(2),
                NewItem::titled("owned")
                    .owned_by("worker")
                    .from_entry(EntryNo(5)),
            )
            .unwrap()
            .value;
        let ask = b
            .open_ask_in(
                &Actor::worker("worker"),
                Ts(3),
                CrewId::new("crew-a"),
                "complete question",
                Some(DefaultOption {
                    text: "default".into(),
                    after_ms: 500,
                }),
                Some(id),
            )
            .unwrap()
            .value;
        let human = b
            .create_human_task(&Actor::Human, Ts(4), CrewId::new("crew-a"), "human task")
            .unwrap()
            .value;
        assert!(matches!(
            b.on_idle(&SessionId::new("worker"), Ts(5)),
            StopDecision::Rewake(_)
        ));
        let delta = b.take_persistent_delta().unwrap();
        delta.validate(&before, b.state()).unwrap();
        assert_eq!(
            delta.changes.entries[&EntryNo(5)].after.as_ref(),
            b.entry(EntryNo(5))
        );
        assert_eq!(delta.changes.asks[&ask].after.as_ref(), b.ask(ask));
        assert_eq!(delta.changes.asks[&human].after.as_ref(), b.ask(human));
        assert_eq!(delta.changes.asks.len(), 2);
        assert!(
            delta.changes.asks[&human]
                .after
                .as_ref()
                .unwrap()
                .human_task
        );
        assert_eq!(
            delta.changes.stop_streaks[&SessionId::new("worker")]
                .after
                .as_ref(),
            b.state().stop_streaks.get(&SessionId::new("worker"))
        );
        let metadata = delta.changes.metadata.as_ref().unwrap();
        assert_eq!(metadata.before.organization, Some(Organization::default()));
        assert_eq!(metadata.after.organization, b.state().organization);
        assert_eq!(metadata.after.last_entry, 5);
        assert_eq!(metadata.after.last_ask, 2);
        assert_eq!(metadata.after.last_seq, b.state().last_seq);
    }

    #[test]
    fn clock_and_idle_noevent_produce_delta() {
        let mut b = book();
        b.register_entry(Ts(1), EntryNo(1)).unwrap();
        create_owned(&mut b);
        b.take_log();
        let before = b.state().clone();
        b.enable_persistent_delta_tracking();
        let duplicate = b.register_entry(Ts(4), EntryNo(1)).unwrap();
        assert!(!duplicate.value);
        assert!(duplicate.events.is_empty());
        assert!(matches!(
            b.on_idle(&SessionId::new("worker"), Ts(5)),
            StopDecision::Rewake(_)
        ));
        assert!(b.log().is_empty());
        assert_eq!(
            b.state().clock,
            Ts(4),
            "idle itself does not advance the clock"
        );
        let delta = b.take_persistent_delta().unwrap();
        delta.validate(&before, b.state()).unwrap();
        assert_eq!(delta.changes.metadata.as_ref().unwrap().after.clock, Ts(4));
        assert!(!delta.changes.stop_streaks.is_empty());
        assert!(delta.changes.items.is_empty());
    }

    #[test]
    fn rejected_mutation_no_delta_or_number() {
        let mut b = book();
        let before = b.state().clone();
        b.enable_persistent_delta_tracking();
        assert!(
            b.create_item(&Actor::Human, Ts(9), NewItem::titled(" "))
                .is_err()
        );
        let late = NewItem {
            next_check_at: Some(Ts(100)),
            ..NewItem::titled("unowned cannot schedule a check")
        };
        assert!(matches!(
            b.create_item(&Actor::Human, Ts(10), late),
            Err(Rejected::Invariant(_))
        ));
        assert_eq!(b.state(), &before);
        assert!(b.log().is_empty());
        let pending = b.pending_persistent_delta().unwrap().clone();
        assert!(pending.is_empty());
        pending.validate(&before, b.state()).unwrap();
        assert_eq!(b.state().last_item, 0);
        assert_eq!(b.state().last_seq, 0);
        b.enable_persistent_delta_tracking();
        assert_eq!(b.pending_persistent_delta(), Some(&pending));
    }

    #[test]
    fn multiple_mutations_first_before_last_after() {
        let mut b = book();
        let id = create_owned(&mut b);
        let before = b.state().clone();
        b.enable_persistent_delta_tracking();
        b.link_channel(&Actor::Human, Ts(2), id, Some(ChannelId::new("first")))
            .unwrap();
        let middle = b.state().clone();
        let first = PersistentDelta::between(&before, &middle);
        b.enable_persistent_delta_tracking();
        assert_eq!(b.pending_persistent_delta(), Some(&first));
        b.link_channel(&Actor::Human, Ts(3), id, Some(ChannelId::new("last")))
            .unwrap();
        let after = b.state().clone();
        let second = PersistentDelta::between(&middle, &after);
        let composed = first.compose(&second, &before, &middle, &after).unwrap();
        let captured = b.take_persistent_delta().unwrap();
        assert_eq!(captured, composed);
        captured.validate(&before, &after).unwrap();
        assert_eq!(
            captured.changes.items[&id].before.as_ref(),
            before.items.get(&id)
        );
        assert_eq!(
            captured.changes.items[&id].after.as_ref(),
            after.items.get(&id)
        );
        assert_eq!(
            captured.changes.items[&id].after.as_ref().unwrap().channel,
            Some(ChannelId::new("last"))
        );
    }

    #[test]
    fn delta_cannot_omit_extra_or_wrongcrew_change() {
        let mut b = WorkBook::organization(Config::default());
        b.set_default_crew(CrewId::new("crew-a"));
        b.register_entry(Ts(1), EntryNo(1)).unwrap();
        let before = b.state().clone();
        let id = b
            .create_item(&Actor::Human, Ts(2), NewItem::titled("new"))
            .unwrap()
            .value;
        let after = b.state().clone();
        let delta = PersistentDelta::between(&before, &after);
        let mut omitted = delta.clone();
        omitted.changes.items.clear();
        assert_eq!(
            omitted.validate(&before, &after),
            Err(DeltaError::ChangesMismatch)
        );
        let mut extra = delta.clone();
        extra.changes.entries.insert(
            EntryNo(1),
            Change {
                before: before.entries.get(&EntryNo(1)).cloned(),
                after: after.entries.get(&EntryNo(1)).cloned(),
            },
        );
        assert_eq!(
            extra.validate(&before, &after),
            Err(DeltaError::ChangesMismatch)
        );
        let mut wrong = delta.clone();
        wrong
            .changes
            .metadata
            .as_mut()
            .unwrap()
            .after
            .organization
            .as_mut()
            .unwrap()
            .item_crews
            .insert(id, CrewId::new("wrong-crew"));
        assert_eq!(
            wrong.validate(&before, &after),
            Err(DeltaError::ChangesMismatch)
        );
        let mut wrong_baseline = before.clone();
        wrong_baseline.crew = CrewId::new("different authority");
        assert_eq!(
            delta.validate(&wrong_baseline, &after),
            Err(DeltaError::BaselineMismatch)
        );
    }

    #[test]
    fn untouched_baseline_and_intermediate_cannot_be_substituted() {
        let mut b = book();
        let id = create_owned(&mut b);
        let before = b.state().clone();
        b.link_channel(&Actor::Human, Ts(2), id, Some(ChannelId::new("next")))
            .unwrap();
        let middle = b.state().clone();
        b.link_channel(&Actor::Human, Ts(3), id, None).unwrap();
        let after = b.state().clone();
        let first = PersistentDelta::between(&before, &middle);
        let second = PersistentDelta::between(&middle, &after);
        let mut wrong = before.clone();
        wrong.last_entry = 500;
        assert_eq!(
            first.validate(&wrong, &middle),
            Err(DeltaError::BaselineMismatch)
        );
        let mut wrong_middle = middle.clone();
        wrong_middle.organization = Some(Organization::default());
        assert!(
            first
                .compose(&second, &before, &wrong_middle, &after)
                .is_err()
        );
        let mut wrong_after = after.clone();
        wrong_after.items.get_mut(&id).unwrap().body.acceptance = "unrecorded".into();
        assert!(
            first
                .compose(&second, &before, &middle, &wrong_after)
                .is_err()
        );
    }

    #[test]
    fn net_zero_entities_keep_real_metadata_and_complete_deletions() {
        let mut b = book();
        let before = b.state().clone();
        let id = create_owned(&mut b);
        let middle = b.state().clone();
        // A pure deletion image for a future canonical purge adapter; no live
        // WorkBook purge consumer is implemented by this module.
        let mut after = middle.clone();
        after.items.remove(&id);
        after.last_seq += 1;
        after.clock = Ts(2);
        let first = PersistentDelta::between(&before, &middle);
        let deleted = PersistentDelta::between(&middle, &after);
        assert_eq!(
            deleted.changes.items[&id].before.as_ref(),
            middle.items.get(&id)
        );
        assert_eq!(deleted.changes.items[&id].after, None);
        let combined = first.compose(&deleted, &before, &middle, &after).unwrap();
        combined.validate(&before, &after).unwrap();
        assert!(combined.changes.items.is_empty());
        let meta = combined.changes.metadata.unwrap();
        assert_eq!(meta.before.last_item, 0);
        assert_eq!(meta.after.last_item, 1);
        assert_eq!(meta.after.last_seq, 2);
        assert_eq!(meta.after.clock, Ts(2));
    }

    #[test]
    fn metadata_none_empty_and_full_integer_domains_are_distinct() {
        // Pure typed representation coverage, not an alternate domain admission.
        let before = BookState::empty(CrewId::new("original"));
        let mut after = before.clone();
        after.crew = CrewId::new("other");
        after.organization = Some(Organization::default());
        after.max_parent_depth = Some(u8::MAX);
        after.last_item = u32::MAX;
        after.last_ask = u32::MAX;
        after.last_entry = i64::MAX;
        after.last_seq = u64::MAX;
        after.clock = Ts(i64::MAX);
        let delta = PersistentDelta::between(&before, &after);
        delta.validate(&before, &after).unwrap();
        let m = delta.changes.metadata.unwrap();
        assert_eq!(m.before.organization, None);
        assert_eq!(m.after.organization, Some(Organization::default()));
        assert_eq!(m.after.max_parent_depth, Some(u8::MAX));
        assert_eq!(m.after.last_item, u32::MAX);
        assert_eq!(m.after.last_ask, u32::MAX);
        assert_eq!(m.after.last_entry, i64::MAX);
        assert_eq!(m.after.last_seq, u64::MAX);
        assert_eq!(m.after.clock, Ts(i64::MAX));
        assert_eq!(m.after.crew, CrewId::new("other"));
    }

    #[test]
    fn idle_clean_removal_is_recorded_without_events() {
        let mut b = book();
        let id = create_owned(&mut b);
        let worker = SessionId::new("worker");
        assert!(matches!(b.on_idle(&worker, Ts(2)), StopDecision::Rewake(_)));
        b.report(
            &Actor::worker("worker"),
            Ts(3),
            id,
            Report {
                status: Some(Progress::Open),
                next_check_at: Some(Ts(100)),
                note: "scheduled".into(),
            },
        )
        .unwrap();
        assert!(b.state().stop_streaks.contains_key(&worker));
        b.take_log();
        let before = b.state().clone();
        b.enable_persistent_delta_tracking();
        assert_eq!(b.on_idle(&worker, Ts(4)), StopDecision::Clean);
        let delta = b.take_persistent_delta().unwrap();
        delta.validate(&before, b.state()).unwrap();
        assert_eq!(
            delta.changes.stop_streaks[&worker].before.as_ref(),
            before.stop_streaks.get(&worker)
        );
        assert_eq!(delta.changes.stop_streaks[&worker].after, None);
        assert!(delta.changes.metadata.is_none());
        assert!(b.log().is_empty());
    }

    #[test]
    fn entry_resolution_and_closure_keep_complete_original_values() {
        let mut b = book();
        let no = EntryNo(9);
        b.register_entry(Ts(1), no).unwrap();
        let before = b.state().clone();
        b.enable_persistent_delta_tracking();
        b.set_entry_resolution(&Actor::Human, Ts(2), no, true)
            .unwrap();
        b.close_entry(
            &Actor::Human,
            Ts(3),
            no,
            "the entire closure note",
            Some(Evidence::Text("the entire closure evidence".into())),
        )
        .unwrap();
        let delta = b.take_persistent_delta().unwrap();
        delta.validate(&before, b.state()).unwrap();
        let change = &delta.changes.entries[&no];
        assert_eq!(change.before.as_ref(), before.entries.get(&no));
        assert_eq!(change.after.as_ref(), b.entry(no));
        let entry = change.after.as_ref().unwrap();
        assert!(entry.manual_done);
        assert_eq!(entry.registered_at, Ts(1));
        let closed = entry.closed.as_ref().unwrap();
        assert_eq!(closed.by, Actor::Human);
        assert_eq!(closed.note, "the entire closure note");
        assert_eq!(
            closed.evidence,
            Some(Evidence::Text("the entire closure evidence".into()))
        );
        assert_eq!(closed.at, Ts(3));
    }

    #[test]
    fn delta_and_event_drains_are_independent_and_reanchor_exactly() {
        let mut b = book();
        let before = b.state().clone();
        b.enable_persistent_delta_tracking();
        create_owned(&mut b);
        let first_after = b.state().clone();
        let original_log = b.log().to_vec();
        let delta = b.take_persistent_delta().unwrap();
        delta.validate(&before, &first_after).unwrap();
        assert_eq!(b.log(), original_log);
        assert!(b.pending_persistent_delta().unwrap().is_empty());
        assert_eq!(b.take_log(), original_log);
        b.create_item(&Actor::Human, Ts(2), NewItem::titled("second"))
            .unwrap();
        let pending = b.pending_persistent_delta().unwrap().clone();
        let second_log = b.take_log();
        assert_eq!(second_log.len(), 1);
        assert_eq!(b.pending_persistent_delta(), Some(&pending));
        pending.validate(&first_after, b.state()).unwrap();
        assert_eq!(b.finish_persistent_delta_tracking(), Some(pending));
        assert!(b.pending_persistent_delta().is_none());
        assert!(b.take_persistent_delta().is_none());
    }

    #[test]
    fn normal_restore_and_clone_keep_explicit_tracking_semantics() {
        let mut normal = book();
        assert!(normal.pending_persistent_delta().is_none());
        let mut tracked = normal.clone();
        tracked.enable_persistent_delta_tracking();
        let left = normal
            .create_item(&Actor::Human, Ts(1), NewItem::titled("same"))
            .unwrap();
        let right = tracked
            .create_item(&Actor::Human, Ts(1), NewItem::titled("same"))
            .unwrap();
        assert_eq!(left, right);
        assert_eq!(normal.state(), tracked.state());
        assert_eq!(normal.log(), tracked.log());
        assert_eq!(
            serde_json::to_vec(normal.state()).unwrap(),
            serde_json::to_vec(tracked.state()).unwrap()
        );
        assert!(normal.take_persistent_delta().is_none());
        let restored = WorkBook::restore(tracked.state().clone(), Config::default()).unwrap();
        assert!(restored.pending_persistent_delta().is_none());
        assert!(!tracked.take_persistent_delta().unwrap().is_empty());
        let mut fork = tracked.clone();
        fork.link_channel(
            &Actor::Human,
            Ts(2),
            left.value,
            Some(ChannelId::new("fork")),
        )
        .unwrap();
        assert_ne!(fork.state(), tracked.state());
        assert!(!fork.pending_persistent_delta().unwrap().is_empty());
        assert!(tracked.pending_persistent_delta().unwrap().is_empty());
        tracked.set_default_crew(CrewId::new("context-only"));
        assert!(tracked.pending_persistent_delta().unwrap().is_empty());
    }

    #[test]
    fn full_ask_and_stop_payload_changes_are_not_summarized() {
        let mut b = book();
        let id = b
            .open_ask(&Actor::worker("worker"), Ts(1), "question", None, None)
            .unwrap()
            .value;
        let before = b.state().clone();
        // Exercise the entire typed payload independently of command admission.
        // Invariants/role restrictions are still the original caller's job.
        let mut after = before.clone();
        let ask = after.asks.get_mut(&id).unwrap();
        ask.human_task = true;
        ask.item = Some(WorkId(2));
        ask.asked_by = Actor::System;
        ask.question = "whole new question".into();
        ask.default = Some(DefaultOption {
            text: "whole option".into(),
            after_ms: 3,
        });
        ask.asked_at = Ts(4);
        ask.state = AskState::Withdrawn {
            reason: "whole reason".into(),
            at: Ts(5),
        };
        ask.escalated = WaitTier::StillWanted;
        ask.standalone = false;
        let sid = SessionId::new("session");
        after.stop_streaks.insert(
            sid.clone(),
            StopStreak {
                last: vec![
                    StopViolation {
                        item: WorkId(7),
                        kind: StopKind::Overdue,
                    },
                    StopViolation {
                        item: WorkId(2),
                        kind: StopKind::Unanswered,
                    },
                ],
                unimproved: 6,
                escalated: true,
                rewakes: 7,
            },
        );
        let delta = PersistentDelta::between(&before, &after);
        delta.validate(&before, &after).unwrap();
        assert_eq!(
            delta.changes.asks[&id].before.as_ref(),
            before.asks.get(&id)
        );
        assert_eq!(delta.changes.asks[&id].after.as_ref(), after.asks.get(&id));
        assert_eq!(
            delta.changes.stop_streaks[&sid].after.as_ref(),
            after.stop_streaks.get(&sid)
        );
        let mut missing = delta.clone();
        missing.changes.stop_streaks.clear();
        assert_eq!(
            missing.validate(&before, &after),
            Err(DeltaError::ChangesMismatch)
        );
    }
}
