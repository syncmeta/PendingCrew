//! 卡住只卡那一件（spec §6）与人回复（spec §5「人回复」）：标 blocked、开人类待确认、拆出能先推进的部分；
//! 人回复 / 默认方案到点 / 卡它的项完成 → 清掉卡点、叫醒负责人。
use crate::book::{Applied, Draft, Rejected, WorkBook};
use crate::events::EventKind;
use crate::ids::{AskId, Ts, WorkId};
use crate::model::{Actor, AskState, Blocker, DefaultOption, HumanAsk, NewItem, Status, WaitTier};
use crate::perm::{Action, Ctx};

/// 卡在什么上。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockOn {
    /// 新开一条人类待确认（H 号由账本发）。
    NewAsk {
        question: String,
        default: Option<DefaultOption>,
    },
    /// 挂到一条已有的、还开着的待确认上。
    Ask(AskId),
    /// 等另一个工作项。
    Item(WorkId),
}

/// 「其余部分能否先推进」——必答（spec §6 ②）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Continuation {
    Nothing,
    /// 拆出子项（parent = 原项，缺省负责人 = 原项负责人）继续推进。
    Split(Vec<NewItem>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockResult {
    pub asks: Vec<AskId>,
    pub split: Vec<WorkId>,
}

impl WorkBook {
    /// `work_item_block(id, on, can_continue_split?)`。
    pub fn block(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
        on: Vec<BlockOn>,
        rest: Continuation,
    ) -> Result<Applied<BlockResult>, Rejected> {
        self.transact(by, now, |d| {
            d.permit_on(Action::Block, id)?;
            if on.is_empty() {
                return Err(Rejected::Invalid("block needs at least one blocker"));
            }
            let mut blockers = Vec::with_capacity(on.len());
            let mut asks = Vec::new();
            for b in on {
                match b {
                    BlockOn::NewAsk { question, default } => {
                        let h = d.new_ask(&question, default, Some(id), false)?;
                        blockers.push(Blocker::Human(h));
                        asks.push(h);
                    }
                    BlockOn::Ask(h) => {
                        let ask = d.s.asks.get(&h).ok_or(Rejected::UnknownAsk(h))?;
                        if ask.state != AskState::Open {
                            return Err(Rejected::AskNotOpen(h));
                        }
                        blockers.push(Blocker::Human(h));
                    }
                    BlockOn::Item(w) => {
                        if d.item(w)?.status.is_terminal() {
                            return Err(Rejected::Terminal(w));
                        }
                        blockers.push(Blocker::Item(w));
                    }
                }
            }
            let split = match rest {
                Continuation::Nothing => Vec::new(),
                Continuation::Split(items) => {
                    if items.is_empty() {
                        return Err(Rejected::Invalid("a split needs at least one item"));
                    }
                    let owner = d.item(id)?.owner.clone();
                    d.insert_batch(items, Some(id), None, owner.as_ref())?
                }
            };
            let it = d.item_mut(id)?;
            for b in &blockers {
                if !it.blocked_on.contains(b) {
                    it.blocked_on.push(*b);
                }
            }
            it.blocked_on.sort();
            it.status = Status::Blocked;
            it.next_check_at = None;
            d.emit(EventKind::Blocked { id, on: blockers });
            Ok(BlockResult { asks, split })
        })
    }

    /// 开一条人类待确认、拿一个 H 号，不卡任何项（裁定 Q1：H 号只由工作账本发；M3 的人类待确认台账用它）。
    /// `item` = 这条问关于哪一项（可不填；填了必须是活着的项，但不会把它标 blocked——要卡住用 `block`）。
    pub fn open_ask(
        &mut self,
        by: &Actor,
        now: Ts,
        question: &str,
        default: Option<DefaultOption>,
        item: Option<WorkId>,
    ) -> Result<Applied<AskId>, Rejected> {
        self.transact(by, now, |d| {
            d.permit(Action::OpenAsk, Ctx::default())?;
            if let Some(w) = item {
                d.live(w)?;
            }
            d.new_ask(question, default, item, true)
        })
    }

    /// 人回复了 H<n>：直接唤醒卡在它上面的所有项的负责人（看事件里的 `BlockerCleared.owner`）。
    /// 返回卡点被清掉的项。
    pub fn answer_ask(
        &mut self,
        by: &Actor,
        now: Ts,
        ask: AskId,
        text: &str,
    ) -> Result<Applied<Vec<WorkId>>, Rejected> {
        self.transact(by, now, |d| {
            let now = d.now;
            d.permit(Action::AnswerAsk, Ctx::default())?;
            let text = text.trim().to_string();
            if text.is_empty() {
                return Err(Rejected::Blank("answer"));
            }
            let a = d.s.asks.get_mut(&ask).ok_or(Rejected::UnknownAsk(ask))?;
            if a.state != AskState::Open {
                return Err(Rejected::AskNotOpen(ask));
            }
            a.state = AskState::Answered {
                text: text.clone(),
                at: now,
            };
            d.emit(EventKind::AskAnswered { ask, text });
            Ok(d.release(Blocker::Human(ask)))
        })
    }

    /// 默认方案到点（调度层调用）：按提问时附的默认方案放行。到点后的动作照常走权限审批，
    /// 触发审批框就不自动执行、转回人（§13 Q2）——那是 M4b 叫醒负责人时写给它的话，不在这里判。
    pub fn apply_default(
        &mut self,
        by: &Actor,
        now: Ts,
        ask: AskId,
    ) -> Result<Applied<Vec<WorkId>>, Rejected> {
        self.transact(by, now, |d| {
            let now = d.now;
            d.permit(Action::ApplyDefault, Ctx::default())?;
            let a = d.s.asks.get_mut(&ask).ok_or(Rejected::UnknownAsk(ask))?;
            if a.state != AskState::Open {
                return Err(Rejected::AskNotOpen(ask));
            }
            let opt = a.default.clone().ok_or(Rejected::NoDefault(ask))?;
            if a.asked_at.plus(opt.after_ms) > now {
                return Err(Rejected::DefaultNotDue(ask));
            }
            a.state = AskState::Defaulted { at: now };
            d.emit(EventKind::AskDefaulted {
                ask,
                text: opt.text,
            });
            Ok(d.release(Blocker::Human(ask)))
        })
    }
}

impl Draft {
    /// 发一个 H 号、记下这条问（`block` 的新问与 `open_ask` 共用）：问题与默认方案先校验，被拒不消耗号码。
    pub(crate) fn new_ask(
        &mut self,
        question: &str,
        default: Option<DefaultOption>,
        item: Option<WorkId>,
        standalone: bool,
    ) -> Result<AskId, Rejected> {
        let question = question.trim().to_string();
        if question.is_empty() {
            return Err(Rejected::Blank("question"));
        }
        if let Some(opt) = &default {
            if opt.text.trim().is_empty() {
                return Err(Rejected::Blank("default option"));
            }
            if opt.after_ms <= 0 {
                return Err(Rejected::Invalid("a default option needs a positive delay"));
            }
        }
        self.s.last_ask += 1;
        let h = AskId(self.s.last_ask);
        self.s.asks.insert(
            h,
            HumanAsk {
                id: h,
                item,
                asked_by: self.by.clone(),
                question: question.clone(),
                default: default.clone(),
                asked_at: self.now,
                state: AskState::Open,
                escalated: WaitTier::Fresh,
                standalone,
            },
        );
        self.emit(EventKind::AskOpened {
            ask: h,
            item,
            question,
            default,
        });
        Ok(h)
    }

    /// 从所有项的 blockedOn 里去掉 `blocker`。卡点清空的项：依赖已齐 → active 且 nextCheckAt = now
    /// （下一轮检查点立刻叫醒负责人）；依赖没齐 → open。每个受影响的项都产出一条 `BlockerCleared`。
    pub(crate) fn release(&mut self, blocker: Blocker) -> Vec<WorkId> {
        let hit: Vec<WorkId> = self
            .s
            .items
            .values()
            .filter(|it| it.blocked_on.contains(&blocker))
            .map(|it| it.id)
            .collect();
        for id in &hit {
            let deps_done = self.s.items[id]
                .depends_on
                .iter()
                .all(|dep| self.s.items[dep].status == Status::Done);
            let now = self.now;
            let it = self.s.items.get_mut(id).expect("collected above");
            it.blocked_on.retain(|b| *b != blocker);
            if it.blocked_on.is_empty() {
                if deps_done {
                    it.status = Status::Active;
                    it.next_check_at = Some(now);
                } else {
                    it.status = Status::Open;
                    it.next_check_at = None;
                }
            }
            let (owner, status) = (it.owner.clone(), it.status);
            self.emit(EventKind::BlockerCleared {
                id: *id,
                owner,
                blocker,
                status,
            });
        }
        hit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{EntryNo, HOUR};
    use crate::invariants::Violation;
    use crate::model::ItemRef;
    use crate::testkit::*;

    fn ask(q: &str) -> BlockOn {
        BlockOn::NewAsk {
            question: q.into(),
            default: None,
        }
    }

    fn ask_with_default(q: &str, text: &str, after_ms: i64) -> BlockOn {
        BlockOn::NewAsk {
            question: q.into(),
            default: Some(DefaultOption {
                text: text.into(),
                after_ms,
            }),
        }
    }

    #[test]
    fn blocking_on_a_new_ask_opens_h1() {
        let mut b = book();
        let w = claim(&mut b, "a", &["迁移"])[0];
        let got = b
            .block(
                &Actor::worker("a"),
                T1,
                w,
                vec![ask(" 要保留旧格式吗？ ")],
                Continuation::Nothing,
            )
            .unwrap();
        assert_eq!(
            got.value,
            BlockResult {
                asks: vec![AskId(1)],
                split: vec![]
            }
        );
        let it = b.item(w).unwrap();
        assert_eq!(it.status, Status::Blocked);
        assert_eq!(it.blocked_on, [Blocker::Human(AskId(1))]);
        assert_eq!(it.next_check_at, None);
        let h = b.ask(AskId(1)).unwrap();
        assert_eq!(h.question, "要保留旧格式吗？");
        assert_eq!(
            (h.item, h.asked_at, h.state.clone(), h.standalone),
            (Some(w), T1, AskState::Open, false)
        );
        assert_eq!(kinds(&got.events), ["ask_opened", "blocked"]);
    }

    #[test]
    fn block_input_is_validated_without_consuming_numbers() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        let a = Actor::worker("a");
        assert_eq!(
            b.block(&a, T1, w, vec![], Continuation::Nothing),
            Err(Rejected::Invalid("block needs at least one blocker"))
        );
        assert_eq!(
            b.block(&a, T1, w, vec![ask("  ")], Continuation::Nothing),
            Err(Rejected::Blank("question"))
        );
        assert_eq!(
            b.block(
                &a,
                T1,
                w,
                vec![ask_with_default("q", "A", 0)],
                Continuation::Nothing
            ),
            Err(Rejected::Invalid("a default option needs a positive delay"))
        );
        assert_eq!(
            b.block(
                &a,
                T1,
                w,
                vec![ask_with_default("q", " ", 5)],
                Continuation::Nothing
            ),
            Err(Rejected::Blank("default option"))
        );
        assert_eq!(
            b.block(
                &a,
                T1,
                w,
                vec![ask("q"), BlockOn::Item(WorkId(9))],
                Continuation::Nothing
            ),
            Err(Rejected::UnknownItem(WorkId(9)))
        );
        assert_eq!(
            b.block(
                &a,
                T1,
                w,
                vec![BlockOn::Ask(AskId(4))],
                Continuation::Nothing
            ),
            Err(Rejected::UnknownAsk(AskId(4)))
        );
        assert_eq!(
            b.block(&a, T1, w, vec![ask("q")], Continuation::Split(vec![])),
            Err(Rejected::Invalid("a split needs at least one item"))
        );
        assert_eq!(b.state().last_ask, 0, "被拒的命令不消耗 H 号");
        let ok = b
            .block(&a, T1, w, vec![ask("q")], Continuation::Nothing)
            .unwrap();
        assert_eq!(ok.value.asks, [AskId(1)]);
    }

    #[test]
    fn split_keeps_the_session_busy_on_the_rest() {
        let mut b = book();
        let w = claim(&mut b, "a", &["整件事"])[0];
        let got = b
            .block(
                &Actor::worker("a"),
                T1,
                w,
                vec![ask("q")],
                Continuation::Split(vec![
                    NewItem::titled("不依赖答案的部分"),
                    NewItem::titled("再下一步").after(ItemRef::Batch(0)),
                ]),
            )
            .unwrap();
        assert_eq!(got.value.split, [WorkId(2), WorkId(3)]);
        let part = b.item(WorkId(2)).unwrap();
        assert_eq!(part.parent, Some(w));
        assert_eq!(part.owner, Some(sid("a")), "缺省仍归原负责人");
        assert_eq!(part.status, Status::Open);
        assert!(part.sources.is_empty(), "拆出的子项不继承入口（裁定 6）");
        assert_eq!(b.item(WorkId(3)).unwrap().depends_on, [WorkId(2)]);
        assert_eq!(
            kinds(&got.events),
            ["ask_opened", "item_created", "item_created", "blocked"]
        );
    }

    #[test]
    fn captain_split_may_route_parts_to_someone_else() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        let got = b
            .block(
                &Actor::captain("c"),
                T1,
                w,
                vec![ask("q")],
                Continuation::Split(vec![NewItem::titled("part").owned_by("b")]),
            )
            .unwrap();
        assert_eq!(b.item(got.value.split[0]).unwrap().owner, Some(sid("b")));
        assert_eq!(
            b.block(
                &Actor::worker("a"),
                T1,
                w,
                vec![ask("q")],
                Continuation::Split(vec![NewItem::titled("p").under(ItemRef::Id(w))])
            ),
            Err(Rejected::Invalid(
                "split items always hang under the blocked item"
            ))
        );
    }

    #[test]
    fn only_owner_or_captain_blocks_and_only_owned_items() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        assert_eq!(
            b.block(
                &Actor::worker("b"),
                T1,
                w,
                vec![ask("q")],
                Continuation::Nothing
            ),
            Err(Rejected::Forbidden(Action::Block))
        );
        assert_eq!(
            b.block(
                &Actor::admin("d"),
                T1,
                w,
                vec![ask("q")],
                Continuation::Nothing
            ),
            Err(Rejected::Forbidden(Action::Block))
        );
        let unowned = b
            .create_item(&Actor::worker("a"), T1, NewItem::titled("orphan"))
            .unwrap()
            .value;
        assert_eq!(
            b.block(
                &Actor::captain("c"),
                T1,
                unowned,
                vec![ask("q")],
                Continuation::Nothing
            ),
            Err(Rejected::Invariant(Violation::OwnerMissing(unowned)))
        );
    }

    #[test]
    fn waiting_on_an_ancestor_is_a_cycle_but_on_a_child_is_fine() {
        let mut b = book();
        let parent = claim(&mut b, "a", &["parent"])[0];
        let child = b
            .create_item(
                &Actor::worker("a"),
                T1,
                NewItem::titled("child")
                    .under(ItemRef::Id(parent))
                    .owned_by("a"),
            )
            .unwrap()
            .value;
        assert_eq!(
            b.block(
                &Actor::worker("a"),
                T1,
                child,
                vec![BlockOn::Item(parent)],
                Continuation::Nothing
            ),
            Err(Rejected::Invariant(Violation::Cycle(vec![parent, child])))
        );
        assert!(
            b.block(
                &Actor::worker("a"),
                T1,
                parent,
                vec![BlockOn::Item(child)],
                Continuation::Nothing
            )
            .is_ok()
        );
    }

    #[test]
    fn human_answer_wakes_the_owner_and_resumes_the_item() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        b.block(
            &Actor::worker("a"),
            T1,
            w,
            vec![ask("q")],
            Continuation::Nothing,
        )
        .unwrap();
        let later = T1.plus(HOUR);
        assert_eq!(
            b.answer_ask(&Actor::captain("c"), later, AskId(1), "yes"),
            Err(Rejected::Forbidden(Action::AnswerAsk)),
            "只有人能回答人类待确认"
        );
        assert_eq!(
            b.answer_ask(&Actor::Human, later, AskId(1), "  "),
            Err(Rejected::Blank("answer"))
        );
        let got = b.answer_ask(&Actor::Human, later, AskId(1), "要").unwrap();
        assert_eq!(got.value, [w]);
        assert_eq!(
            got.events[1].kind,
            EventKind::BlockerCleared {
                id: w,
                owner: Some(sid("a")),
                blocker: Blocker::Human(AskId(1)),
                status: Status::Active
            }
        );
        let it = b.item(w).unwrap();
        assert_eq!((it.status, it.next_check_at), (Status::Active, Some(later)));
        assert!(it.blocked_on.is_empty());
        assert_eq!(
            b.ask(AskId(1)).unwrap().state,
            AskState::Answered {
                text: "要".into(),
                at: later
            }
        );
        assert_eq!(
            b.answer_ask(&Actor::Human, later, AskId(1), "again"),
            Err(Rejected::AskNotOpen(AskId(1)))
        );
        assert_eq!(
            b.answer_ask(&Actor::Human, later, AskId(7), "x"),
            Err(Rejected::UnknownAsk(AskId(7)))
        );
    }

    #[test]
    fn answer_on_one_of_two_blockers_still_wakes_the_owner() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        b.block(
            &Actor::worker("a"),
            T1,
            w,
            vec![ask("q1"), ask("q2")],
            Continuation::Nothing,
        )
        .unwrap();
        let got = b.answer_ask(&Actor::Human, T1, AskId(2), "ok").unwrap();
        assert_eq!(
            got.events[1].kind,
            EventKind::BlockerCleared {
                id: w,
                owner: Some(sid("a")),
                blocker: Blocker::Human(AskId(2)),
                status: Status::Blocked
            }
        );
        assert_eq!(b.item(w).unwrap().blocked_on, [Blocker::Human(AskId(1))]);
    }

    #[test]
    fn unblocked_item_with_unfinished_deps_goes_back_to_open() {
        let mut b = book();
        let ids = b
            .claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![
                    NewItem::titled("dep"),
                    NewItem::titled("x").after(ItemRef::Batch(0)),
                ],
            )
            .unwrap()
            .value;
        b.block(
            &Actor::worker("a"),
            T1,
            ids[1],
            vec![ask("q")],
            Continuation::Nothing,
        )
        .unwrap();
        b.answer_ask(&Actor::Human, T1, AskId(1), "ok").unwrap();
        let it = b.item(ids[1]).unwrap();
        assert_eq!((it.status, it.next_check_at), (Status::Open, None));
    }

    #[test]
    fn one_answer_releases_every_item_waiting_on_it() {
        let mut b = book();
        let ids = claim(&mut b, "a", &["x", "y"]);
        b.block(
            &Actor::worker("a"),
            T1,
            ids[0],
            vec![ask("q")],
            Continuation::Nothing,
        )
        .unwrap();
        b.block(
            &Actor::worker("a"),
            T1,
            ids[1],
            vec![BlockOn::Ask(AskId(1))],
            Continuation::Nothing,
        )
        .unwrap();
        let got = b.answer_ask(&Actor::Human, T1, AskId(1), "ok").unwrap();
        assert_eq!(got.value, ids);
        assert_eq!(
            b.block(
                &Actor::worker("a"),
                T1,
                ids[0],
                vec![BlockOn::Ask(AskId(1))],
                Continuation::Nothing
            ),
            Err(Rejected::AskNotOpen(AskId(1)))
        );
    }

    #[test]
    fn default_option_applies_only_when_due_and_only_by_the_scheduler() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        b.block(
            &Actor::worker("a"),
            T1,
            w,
            vec![
                ask_with_default("用哪个端口？", "用 8080", 4 * HOUR),
                ask("q2"),
            ],
            Continuation::Nothing,
        )
        .unwrap();
        assert_eq!(
            b.apply_default(&Actor::System, T1.plus(4 * HOUR - 1), AskId(1)),
            Err(Rejected::DefaultNotDue(AskId(1)))
        );
        assert_eq!(
            b.apply_default(&Actor::Human, T1.plus(4 * HOUR), AskId(1)),
            Err(Rejected::Forbidden(Action::ApplyDefault))
        );
        assert_eq!(
            b.apply_default(&Actor::System, T1.plus(4 * HOUR), AskId(2)),
            Err(Rejected::NoDefault(AskId(2)))
        );
        let got = b
            .apply_default(&Actor::System, T1.plus(4 * HOUR), AskId(1))
            .unwrap();
        assert_eq!(got.value, [w]);
        assert_eq!(
            got.events[0].kind,
            EventKind::AskDefaulted {
                ask: AskId(1),
                text: "用 8080".into()
            }
        );
        assert_eq!(
            b.ask(AskId(1)).unwrap().state,
            AskState::Defaulted {
                at: T1.plus(4 * HOUR)
            }
        );
        assert_eq!(
            b.answer_ask(&Actor::Human, T1.plus(5 * HOUR), AskId(1), "晚了"),
            Err(Rejected::AskNotOpen(AskId(1)))
        );
    }

    #[test]
    fn a_question_need_not_be_tied_to_a_work_item() {
        // M3 的人类待确认台账也从这里拿 H 号（裁定 Q1：H 号只由工作账本发），问题可以不挂任何工作项。
        let mut b = book();
        let got = b
            .open_ask(
                &Actor::captain("c"),
                T1,
                "  发版窗口定在周几？ ",
                None,
                None,
            )
            .unwrap();
        assert_eq!(got.value, AskId(1));
        assert_eq!(kinds(&got.events), ["ask_opened"]);
        let h = b.ask(AskId(1)).unwrap();
        assert_eq!(
            (h.item, h.question.as_str(), h.state.clone(), h.standalone),
            (None, "发版窗口定在周几？", AskState::Open, true)
        );
        let waits = b.human_waits(T1);
        assert_eq!(waits.len(), 1, "没卡任何项，但有人在等答案：进人的清单");
        assert!(waits[0].items.is_empty());
        assert_eq!(waits[0].owners, [sid("c")]);

        let w = claim(&mut b, "a", &["x"])[0];
        let blocked = b
            .block(
                &Actor::worker("a"),
                T1,
                w,
                vec![ask("q")],
                Continuation::Nothing,
            )
            .unwrap();
        assert_eq!(blocked.value.asks, [AskId(2)], "与卡住时开的问同一个计数器");
        assert_eq!(b.ask(AskId(2)).unwrap().item, Some(w));

        let about = b
            .open_ask(
                &Actor::System,
                T1,
                "要不要顺手升级依赖？",
                Some(DefaultOption {
                    text: "不升".into(),
                    after_ms: HOUR,
                }),
                Some(w),
            )
            .unwrap()
            .value;
        assert_eq!(about, AskId(3));
        assert_eq!(
            b.item(w).unwrap().blocked_on,
            [Blocker::Human(AskId(2))],
            "挂在一项上的问不卡住它"
        );
        assert!(
            b.answer_ask(&Actor::Human, T1, AskId(1), "周四")
                .unwrap()
                .value
                .is_empty(),
            "回答不放行任何项"
        );
        assert_eq!(
            b.open_ask(&Actor::Human, T1, "q", None, None),
            Err(Rejected::Forbidden(Action::OpenAsk))
        );
        assert_eq!(
            b.open_ask(&Actor::worker("a"), T1, " ", None, None),
            Err(Rejected::Blank("question"))
        );
        assert_eq!(
            b.open_ask(&Actor::worker("a"), T1, "q", None, Some(WorkId(9))),
            Err(Rejected::UnknownItem(WorkId(9)))
        );
        assert_eq!(
            b.open_ask(
                &Actor::worker("a"),
                T1,
                "q",
                Some(DefaultOption {
                    text: "A".into(),
                    after_ms: 0
                }),
                None
            ),
            Err(Rejected::Invalid("a default option needs a positive delay"))
        );
        assert_eq!(b.state().last_ask, 3, "被拒的不消耗 H 号");
    }
}
