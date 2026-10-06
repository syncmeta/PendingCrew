//! 谁能做什么（spec §4.2「只有负责人、机长、行政处能改状态」、§5 换负责人、§8.2 行政无业务决策权）。
//! 一张表、一个函数；命令先问它，再做别的检查。
use crate::ids::SessionId;
use crate::model::{Actor, Role};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    ClaimEntry,
    CreateItem,
    /// 在已存在的项下面建子项（ctx.owner = 父项负责人）。
    CreateChild,
    Report,
    Block,
    Done,
    /// 业务上的「不做了」。
    DropDecision,
    /// 重复 / 被取代，带证据。
    DropFactual,
    Relink,
    LinkChannel,
    TransferRequest,
    TransferAccept,
    TransferWithdraw,
    /// 指派负责人（ctx.assignee = 被指派的人）。
    Assign,
    ReassignLost,
    AnswerAsk,
    ApplyDefault,
    RecordWake,
    NoteEscalation,
    CloseEntryDecision,
    CloseEntryFactual,
}

impl Action {
    pub const ALL: [Action; 21] = [
        Action::ClaimEntry,
        Action::CreateItem,
        Action::CreateChild,
        Action::Report,
        Action::Block,
        Action::Done,
        Action::DropDecision,
        Action::DropFactual,
        Action::Relink,
        Action::LinkChannel,
        Action::TransferRequest,
        Action::TransferAccept,
        Action::TransferWithdraw,
        Action::Assign,
        Action::ReassignLost,
        Action::AnswerAsk,
        Action::ApplyDefault,
        Action::RecordWake,
        Action::NoteEscalation,
        Action::CloseEntryDecision,
        Action::CloseEntryFactual,
    ];
}

/// 判定需要的上下文：被操作项当前的负责人、待接受的转交对象、被指派的人。
#[derive(Debug, Clone, Copy, Default)]
pub struct Ctx<'a> {
    pub owner: Option<&'a SessionId>,
    pub transfer_to: Option<&'a SessionId>,
    pub assignee: Option<&'a SessionId>,
}

pub fn allowed(actor: &Actor, action: Action, ctx: Ctx<'_>) -> bool {
    use Action::*;
    match actor {
        Actor::System => matches!(
            action,
            ClaimEntry
                | CreateItem
                | CreateChild
                | Assign
                | ReassignLost
                | ApplyDefault
                | RecordWake
                | NoteEscalation
        ),
        Actor::Human => !matches!(
            action,
            TransferRequest | TransferAccept | ApplyDefault | RecordWake | NoteEscalation
        ),
        Actor::Session { id, role } => {
            let owns = ctx.owner == Some(id);
            match action {
                TransferRequest => owns,
                TransferAccept => ctx.transfer_to == Some(id),
                TransferWithdraw => owns || ctx.transfer_to == Some(id) || *role == Role::Captain,
                AnswerAsk | ApplyDefault | RecordWake | NoteEscalation => false,
                _ => match role {
                    Role::Captain => true,
                    Role::Admin => matches!(
                        action,
                        Done | DropFactual
                            | Relink
                            | LinkChannel
                            | ReassignLost
                            | CloseEntryFactual
                    ),
                    Role::Worker => match action {
                        ClaimEntry | CreateItem => true,
                        CreateChild | Report | Block | Done | DropDecision | DropFactual
                        | Relink | LinkChannel => owns,
                        Assign => ctx.owner.is_none() && ctx.assignee == Some(id),
                        _ => false,
                    },
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Action::*;

    fn sid(s: &str) -> SessionId {
        SessionId::new(s)
    }

    /// 某个操作者在「它是负责人 / 不是负责人」两种上下文下能做的操作集合。
    fn grants(actor: &Actor, ctx: Ctx<'_>) -> Vec<Action> {
        Action::ALL
            .into_iter()
            .filter(|a| allowed(actor, *a, ctx))
            .collect()
    }

    #[test]
    fn worker_acts_only_on_what_it_owns() {
        let me = sid("w");
        let other = sid("x");
        let owner_ctx = Ctx {
            owner: Some(&me),
            ..Ctx::default()
        };
        assert_eq!(
            grants(&Actor::worker("w"), owner_ctx),
            [
                ClaimEntry,
                CreateItem,
                CreateChild,
                Report,
                Block,
                Done,
                DropDecision,
                DropFactual,
                Relink,
                LinkChannel,
                TransferRequest,
                TransferWithdraw
            ]
        );
        let foreign = Ctx {
            owner: Some(&other),
            ..Ctx::default()
        };
        assert_eq!(
            grants(&Actor::worker("w"), foreign),
            [ClaimEntry, CreateItem]
        );
    }

    #[test]
    fn worker_may_assign_only_an_unowned_item_to_itself() {
        let me = sid("w");
        let other = sid("x");
        let w = Actor::worker("w");
        let self_claim = Ctx {
            assignee: Some(&me),
            ..Ctx::default()
        };
        assert!(allowed(&w, Assign, self_claim));
        let to_other = Ctx {
            assignee: Some(&other),
            ..Ctx::default()
        };
        assert!(!allowed(&w, Assign, to_other));
        let owned = Ctx {
            owner: Some(&other),
            assignee: Some(&me),
            ..Ctx::default()
        };
        assert!(!allowed(&w, Assign, owned), "抢别人的项不行");
    }

    #[test]
    fn transfer_target_may_accept_or_decline() {
        let me = sid("w");
        let other = sid("x");
        let ctx = Ctx {
            owner: Some(&other),
            transfer_to: Some(&me),
            ..Ctx::default()
        };
        assert_eq!(
            grants(&Actor::worker("w"), ctx),
            [ClaimEntry, CreateItem, TransferAccept, TransferWithdraw]
        );
    }

    #[test]
    fn captain_has_business_authority_but_not_system_duties() {
        let other = sid("x");
        let ctx = Ctx {
            owner: Some(&other),
            ..Ctx::default()
        };
        let denied: Vec<Action> = Action::ALL
            .into_iter()
            .filter(|a| !allowed(&Actor::captain("c"), *a, ctx))
            .collect();
        assert_eq!(
            denied,
            [
                TransferRequest,
                TransferAccept,
                AnswerAsk,
                ApplyDefault,
                RecordWake,
                NoteEscalation
            ]
        );
    }

    #[test]
    fn admin_closes_only_on_facts() {
        let other = sid("x");
        let ctx = Ctx {
            owner: Some(&other),
            ..Ctx::default()
        };
        assert_eq!(
            grants(&Actor::admin("a"), ctx),
            [
                Done,
                DropFactual,
                Relink,
                LinkChannel,
                ReassignLost,
                CloseEntryFactual
            ]
        );
        assert!(
            !allowed(&Actor::admin("a"), Assign, ctx),
            "强制指派是机长权力"
        );
        assert!(
            !allowed(&Actor::admin("a"), DropDecision, ctx),
            "不能以「不值得做」关闭"
        );
    }

    #[test]
    fn human_is_above_captain_and_is_the_only_one_who_answers() {
        let denied: Vec<Action> = Action::ALL
            .into_iter()
            .filter(|a| !allowed(&Actor::Human, *a, Ctx::default()))
            .collect();
        assert_eq!(
            denied,
            [
                TransferRequest,
                TransferAccept,
                ApplyDefault,
                RecordWake,
                NoteEscalation
            ]
        );
        let answerers: Vec<Actor> = [
            Actor::worker("w"),
            Actor::captain("c"),
            Actor::admin("a"),
            Actor::Human,
            Actor::System,
        ]
        .into_iter()
        .filter(|a| allowed(a, AnswerAsk, Ctx::default()))
        .collect();
        assert_eq!(answerers, [Actor::Human]);
    }

    #[test]
    fn system_only_dispatches_and_keeps_time() {
        assert_eq!(
            grants(&Actor::System, Ctx::default()),
            [
                ClaimEntry,
                CreateItem,
                CreateChild,
                Assign,
                ReassignLost,
                ApplyDefault,
                RecordWake,
                NoteEscalation
            ]
        );
    }
}
