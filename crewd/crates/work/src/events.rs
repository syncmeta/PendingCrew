//! 变更事件。每条成功的命令产出一串事件（同一个时间、同一个操作者、连续的序号），
//! M4b 照着它落库、推给订阅者、决定叫醒谁；被拒的命令不产出任何事件。
use crate::ids::{AskId, ChannelId, EntryNo, SessionId, Ts, WorkId};
use crate::model::{
    Actor, Blocker, DefaultOption, DropReason, Evidence, HowChanged, Status, WaitTier,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub at: Ts,
    pub by: Actor,
    pub kind: EventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    EntryRegistered {
        entry: EntryNo,
    },
    EntryClaimed {
        entry: EntryNo,
        items: Vec<WorkId>,
    },
    EntryClosed {
        entry: EntryNo,
        note: String,
    },
    ItemCreated {
        id: WorkId,
        title: String,
        parent: Option<WorkId>,
        depends_on: Vec<WorkId>,
        sources: Vec<EntryNo>,
        owner: Option<SessionId>,
    },
    Reported {
        id: WorkId,
        from: Status,
        to: Status,
        next_check_at: Option<Ts>,
        note: String,
    },
    Blocked {
        id: WorkId,
        on: Vec<Blocker>,
    },
    AskOpened {
        ask: AskId,
        item: Option<WorkId>,
        question: String,
        default: Option<DefaultOption>,
    },
    AskAnswered {
        ask: AskId,
        text: String,
    },
    AskDefaulted {
        ask: AskId,
        text: String,
    },
    AskWithdrawn {
        ask: AskId,
        reason: String,
    },
    /// 某项的一个卡点消失了（人回复了 / 默认方案到点 / 卡它的项完成了）。
    /// `owner` 要被直接叫醒（spec §5「人回复」），不论这一项是否还卡在别处。
    BlockerCleared {
        id: WorkId,
        owner: Option<SessionId>,
        blocker: Blocker,
        status: Status,
    },
    Done {
        id: WorkId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        evidence: Option<Evidence>,
    },
    Dropped {
        id: WorkId,
        reason: DropReason,
    },
    Relinked {
        id: WorkId,
        parent: Option<WorkId>,
        depends_on: Vec<WorkId>,
    },
    ChannelLinked {
        id: WorkId,
        channel: Option<ChannelId>,
    },
    TransferRequested {
        id: WorkId,
        to: SessionId,
        note: String,
    },
    TransferWithdrawn {
        id: WorkId,
    },
    /// 换了负责人。`channel` 是该项关联的群：M4b 要把新负责人拉进去（spec §17）。
    OwnerChanged {
        id: WorkId,
        from: Option<SessionId>,
        to: SessionId,
        how: HowChanged,
        note: String,
        channel: Option<ChannelId>,
    },
    CheckpointWake {
        id: WorkId,
        owner: SessionId,
        missed_checks: u32,
        next_check_at: Option<Ts>,
    },
    EscalationNoted {
        ask: AskId,
        tier: WaitTier,
    },
}

#[cfg(test)]
mod completion_codec_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn legacy_evidence_and_omitted_evidence_are_both_readable_without_fabrication() {
        let old = json!({"type":"done","id":"W1","evidence":{"kind":"text","value":"existing"}});
        let done: EventKind = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(serde_json::to_value(done).unwrap(), old);
        let bare = json!({"type":"done","id":"W1"});
        let done: EventKind = serde_json::from_value(bare.clone()).unwrap();
        assert!(matches!(&done, EventKind::Done { evidence: None, .. }));
        assert_eq!(serde_json::to_value(done).unwrap(), bare);
    }
}
