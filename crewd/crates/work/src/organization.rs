//! Explicit legacy identity translation into a single organization authority.
//! This pure stage writes no store, changes no external handles, and invents no events.
use crate::invariants::{self, Violation};
use crate::{AskId, Blocker, BookState, CrewId, EntryNo, Organization, StopViolation, WorkId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyIdentityMap {
    pub items: BTreeMap<WorkId, WorkId>,
    pub asks: BTreeMap<AskId, AskId>,
    pub entries: BTreeMap<EntryNo, EntryNo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyMerge {
    pub state: BookState,
    /// Identity index only; never another writable work ledger.
    pub identities: BTreeMap<CrewId, LegacyIdentityMap>,
    /// Existing crew event sequences remain in their original evidence stream.
    pub legacy_sequences: BTreeMap<CrewId, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationError {
    DuplicateCrew(CrewId),
    InvalidCrew(CrewId),
    InvalidIdentity { crew: CrewId, kind: &'static str },
    AlreadyOrganization(CrewId),
    InvalidBook { crew: CrewId, violation: Violation },
    Exhausted { crew: CrewId, kind: &'static str },
    InvalidOrganization(Violation),
}
impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateCrew(c) => write!(f, "duplicate legacy crew {c}"),
            Self::InvalidCrew(c) => write!(f, "blank legacy crew {c}"),
            Self::InvalidIdentity { crew, kind } => {
                write!(f, "legacy crew {crew}: nonpositive {kind} identity")
            }
            Self::AlreadyOrganization(c) => write!(f, "{c} is already an organization book"),
            Self::InvalidBook { crew, violation } => write!(f, "legacy crew {crew}: {violation}"),
            Self::Exhausted { crew, kind } => {
                write!(f, "legacy crew {crew}: {kind} identity range exhausted")
            }
            Self::InvalidOrganization(v) => write!(f, "merged organization: {v}"),
        }
    }
}
impl std::error::Error for MigrationError {}

/// Deterministic crew ordering makes retries stable. Each legacy issued range is
/// reserved in full so a missing/deleted historical number can never be reused.
/// Callers must save this result and update external identity references in one transaction.
pub fn merge_legacy_books(books: Vec<BookState>) -> Result<LegacyMerge, MigrationError> {
    let mut sources = BTreeMap::new();
    for s in books {
        let crew = s.crew.clone();
        if crew.as_str().trim().is_empty() {
            return Err(MigrationError::InvalidCrew(crew));
        }
        if s.organization.is_some() {
            return Err(MigrationError::AlreadyOrganization(crew));
        }
        if sources.insert(crew.clone(), s).is_some() {
            return Err(MigrationError::DuplicateCrew(crew));
        }
    }
    let mut state = BookState::empty(CrewId::new("organization"));
    state.organization = Some(Organization::default());
    state.max_parent_depth = Some(3);
    let mut identities = BTreeMap::new();
    let mut legacy_sequences = BTreeMap::new();
    let mut entry_offset = 0_i64;
    for (crew, s) in sources {
        invariants::check(&s)
            .and_then(|()| invariants::check_parent_depth(&s, 3))
            .map_err(|violation| MigrationError::InvalidBook {
                crew: crew.clone(),
                violation,
            })?;
        for (invalid, kind) in [
            (s.items.keys().any(|w| w.0 == 0), "W"),
            (s.asks.keys().any(|h| h.0 == 0), "H"),
            (s.entries.keys().any(|e| e.0 <= 0), "#"),
        ] {
            if invalid {
                return Err(MigrationError::InvalidIdentity {
                    crew: crew.clone(),
                    kind,
                });
            }
        }
        // Legacy restore did not validate soft stop manifests; reject dangling
        // issued identities explicitly rather than panicking or erasing evidence.
        for streak in s.stop_streaks.values() {
            for v in &streak.last {
                if !s.items.contains_key(&v.item) {
                    return Err(MigrationError::InvalidBook {
                        crew: crew.clone(),
                        violation: Violation::UnknownItemRef {
                            item: v.item,
                            missing: v.item,
                        },
                    });
                }
            }
        }
        let exhausted = |kind| MigrationError::Exhausted {
            crew: crew.clone(),
            kind,
        };
        let item_offset = state.last_item;
        let ask_offset = state.last_ask;
        state.last_item = item_offset
            .checked_add(s.last_item)
            .ok_or_else(|| exhausted("W"))?;
        state.last_ask = ask_offset
            .checked_add(s.last_ask)
            .ok_or_else(|| exhausted("H"))?;
        let last_entry = s
            .entries
            .keys()
            .next_back()
            .map_or(0, |id| id.0)
            .max(s.last_entry);
        let next_entry_offset = entry_offset
            .checked_add(last_entry)
            .ok_or_else(|| exhausted("#"))?;
        let map = LegacyIdentityMap {
            items: s
                .items
                .keys()
                .map(|id| (*id, WorkId(item_offset + id.0)))
                .collect(),
            asks: s
                .asks
                .keys()
                .map(|id| (*id, AskId(ask_offset + id.0)))
                .collect(),
            entries: s
                .entries
                .keys()
                .map(|id| (*id, EntryNo(entry_offset + id.0)))
                .collect(),
        };
        state.clock = state.clock.max(s.clock);
        legacy_sequences.insert(crew.clone(), s.last_seq);
        for (id, mut item) in s.items {
            item.id = map.items[&id];
            item.parent = item.parent.map(|p| map.items[&p]);
            item.depends_on = item.depends_on.iter().map(|p| map.items[p]).collect();
            item.sources = item.sources.iter().map(|p| map.entries[p]).collect();
            item.blocked_on = item
                .blocked_on
                .iter()
                .map(|b| match b {
                    Blocker::Item(w) => Blocker::Item(map.items[w]),
                    Blocker::Human(h) => Blocker::Human(map.asks[h]),
                })
                .collect();
            state
                .organization
                .as_mut()
                .expect("initialized")
                .item_crews
                .insert(item.id, crew.clone());
            state.items.insert(item.id, item);
        }
        for (id, mut ask) in s.asks {
            ask.id = map.asks[&id];
            ask.item = ask.item.map(|p| map.items[&p]);
            state
                .organization
                .as_mut()
                .expect("initialized")
                .ask_crews
                .insert(ask.id, crew.clone());
            state.asks.insert(ask.id, ask);
        }
        for (id, mut entry) in s.entries {
            entry.no = map.entries[&id];
            state
                .organization
                .as_mut()
                .expect("initialized")
                .entry_crews
                .insert(entry.no, crew.clone());
            state.entries.insert(entry.no, entry);
        }
        for (sid, mut streak) in s.stop_streaks {
            streak.last = streak
                .last
                .iter()
                .map(|v| StopViolation {
                    item: map.items[&v.item],
                    kind: v.kind,
                })
                .collect();
            state
                .stop_streaks
                .entry(sid)
                .and_modify(|current| {
                    current.last.extend(streak.last.iter().copied());
                    current.last.sort();
                    current.last.dedup();
                    current.unimproved = current.unimproved.max(streak.unimproved);
                    current.rewakes = current.rewakes.max(streak.rewakes);
                    current.escalated |= streak.escalated;
                })
                .or_insert(streak);
        }
        identities.insert(crew, map);
        entry_offset = next_entry_offset;
        state.last_entry = entry_offset;
    }
    // last_seq = 0 describes only the new organization event stream. No Created
    // events are fabricated; old sequences and event payloads remain source evidence.
    invariants::check(&state).map_err(MigrationError::InvalidOrganization)?;
    Ok(LegacyMerge {
        state,
        identities,
        legacy_sequences,
    })
}
