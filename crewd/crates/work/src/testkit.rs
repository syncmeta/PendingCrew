//! 单测共用的夹具（只在 `cfg(test)` 下编译）。
use crate::book::WorkBook;
use crate::events::{Event, EventKind};
use crate::ids::{CrewId, EntryNo, HOUR, SessionId, Ts, WorkId};
use crate::model::{Actor, Config, NewItem};
use crate::progress::{Progress, Report};

pub(crate) const T0: Ts = Ts(1_700_000_000_000);
/// T0 之后一分钟。
pub(crate) const T1: Ts = Ts(1_700_000_060_000);

pub(crate) fn sid(s: &str) -> SessionId {
    SessionId::new(s)
}

/// 一个机组账本，入口 #1 已在 T0 登记。
pub(crate) fn book() -> WorkBook {
    let mut b = WorkBook::new(CrewId::new("crew-1"), Config::default());
    b.register_entry(T0, EntryNo(1)).unwrap();
    b
}

/// `who`（worker）在 T1 认领 #1，按 `titles` 建项（互不相连），返回 W 号。
pub(crate) fn claim(b: &mut WorkBook, who: &str, titles: &[&str]) -> Vec<WorkId> {
    let items = titles.iter().map(|t| NewItem::titled(t)).collect();
    b.claim_entry(&Actor::worker(who), T1, EntryNo(1), items)
        .unwrap()
        .value
}

pub(crate) fn kind_name(k: &EventKind) -> String {
    serde_json::to_value(k).unwrap()["type"]
        .as_str()
        .unwrap()
        .to_string()
}

pub(crate) fn kinds(events: &[Event]) -> Vec<String> {
    events.iter().map(|e| kind_name(&e.kind)).collect()
}

/// `who` 在 `at` 把 `id` 改成 active，下次检查在 `at` 之后一小时。
pub(crate) fn activate(b: &mut WorkBook, who: &str, id: WorkId, at: Ts) {
    b.report(
        &Actor::worker(who),
        at,
        id,
        Report {
            status: Some(Progress::Active),
            next_check_at: Some(at.plus(HOUR)),
            note: "开工".into(),
        },
    )
    .unwrap();
}
