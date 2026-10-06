//! 一个机组的工作账本（`WorkBook`）：命令的事务外壳，加上入口登记、认领入口、建项。
//! 每条命令在整份状态的副本（草稿）上改，改完整份跑不变量，过了才换上并产出事件；
//! 被拒的命令不改任何东西（号码也不消耗）。
use crate::events::{Event, EventKind};
use crate::ids::{AskId, ChannelId, CrewId, EntryNo, SessionId, Ts, WorkId};
use crate::invariants::{self, Violation};
use crate::model::{
    Actor, BookState, Config, EntryTodo, HowChanged, HumanAsk, ItemRef, NewItem, OwnerChange,
    Status, WorkItem,
};
use crate::perm::{self, Action, Ctx};

/// 命令被拒的原因。被拒 = 账本原样不动。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    Forbidden(Action),
    UnknownItem(WorkId),
    UnknownEntry(EntryNo),
    UnknownAsk(AskId),
    /// 该项已 done / dropped。
    Terminal(WorkId),
    EntryClosed(EntryNo),
    /// 必填的文字是空的（字段名）。
    Blank(&'static str),
    /// 同一批里引用了不在它前面的第几条。
    BatchRef(usize),
    /// 认领入口建出的项必须有负责人。
    OwnerRequired,
    NextCheckNotFuture,
    SameOwner,
    TransferPending(WorkId),
    NoPendingTransfer(WorkId),
    NotOwned(WorkId),
    AskNotOpen(AskId),
    NoDefault(AskId),
    DefaultNotDue(AskId),
    Invalid(&'static str),
    Invariant(Violation),
}

/// 成功的命令：返回值 + 这次产出的事件（已追加到账本的事件日志里）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied<T> {
    pub value: T,
    pub events: Vec<Event>,
}

#[derive(Debug, Clone)]
pub struct WorkBook {
    pub(crate) state: BookState,
    pub(crate) cfg: Config,
    log: Vec<Event>,
}

/// 草稿：命令在它上面改；`transact` 负责校验与提交。
pub(crate) struct Draft {
    pub(crate) s: BookState,
    pub(crate) now: Ts,
    pub(crate) by: Actor,
    events: Vec<EventKind>,
}

impl WorkBook {
    pub fn new(crew: CrewId, cfg: Config) -> WorkBook {
        WorkBook {
            state: BookState::empty(crew),
            cfg,
            log: Vec::new(),
        }
    }

    /// 从持久化读回（M4b）。整份跑一遍不变量，不合法就拒绝，不做任何修补。
    pub fn restore(state: BookState, cfg: Config, now: Ts) -> Result<WorkBook, Violation> {
        invariants::check(&state, &cfg, now.max(state.clock))?;
        Ok(WorkBook {
            state,
            cfg,
            log: Vec::new(),
        })
    }

    pub fn state(&self) -> &BookState {
        &self.state
    }
    pub fn config(&self) -> &Config {
        &self.cfg
    }
    pub fn crew(&self) -> &CrewId {
        &self.state.crew
    }
    pub fn item(&self, id: WorkId) -> Option<&WorkItem> {
        self.state.items.get(&id)
    }
    pub fn ask(&self, id: AskId) -> Option<&HumanAsk> {
        self.state.asks.get(&id)
    }
    pub fn entry(&self, no: EntryNo) -> Option<&EntryTodo> {
        self.state.entries.get(&no)
    }
    /// 自上次 `take_log` 以来的全部事件（按序号）。
    pub fn log(&self) -> &[Event] {
        &self.log
    }
    /// 取走事件日志（M4b 落库后调用）。
    pub fn take_log(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.log)
    }

    pub(crate) fn transact<T>(
        &mut self,
        by: &Actor,
        now: Ts,
        f: impl FnOnce(&mut Draft) -> Result<T, Rejected>,
    ) -> Result<Applied<T>, Rejected> {
        // 命令一律用 d.now（= max(now, clock)），不用调用方传进来的原值。
        let now = now.max(self.state.clock);
        let mut d = Draft {
            s: self.state.clone(),
            now,
            by: by.clone(),
            events: Vec::new(),
        };
        d.s.clock = now;
        let value = f(&mut d)?;
        let me = by.session();
        for (id, it) in d.s.items.iter_mut() {
            let old = self.state.items.get(id);
            if old != Some(&*it) && me.is_some() && it.owner.as_ref() == me {
                // 负责人对自己名下项的任何成功操作都算一次回报（裁定 9）。
                it.missed_checks = 0;
            }
            match old {
                Some(old) if old == it => {}
                Some(old) => {
                    it.revision = old.revision + 1;
                    it.updated_at = now;
                }
                None => {
                    it.revision = 1;
                    it.created_at = now;
                    it.updated_at = now;
                }
            }
        }
        invariants::check(&d.s, &self.cfg, now).map_err(Rejected::Invariant)?;
        let mut events = Vec::with_capacity(d.events.len());
        for kind in d.events {
            d.s.last_seq += 1;
            events.push(Event {
                seq: d.s.last_seq,
                at: now,
                by: by.clone(),
                kind,
            });
        }
        self.state = d.s;
        self.log.extend(events.iter().cloned());
        Ok(Applied { value, events })
    }

    /// crewd 收到一条新的入口 Todo（#N）时登记。重复登记不报错、不产出事件（重启后对账用）。
    pub fn register_entry(&mut self, now: Ts, no: EntryNo) -> Result<Applied<bool>, Rejected> {
        self.transact(&Actor::System, now, |d| {
            if d.s.entries.contains_key(&no) {
                return Ok(false);
            }
            d.s.entries.insert(
                no,
                EntryTodo {
                    no,
                    registered_at: d.now,
                    closed: None,
                },
            );
            d.emit(EventKind::EntryRegistered { entry: no });
            Ok(true)
        })
    }

    /// `claim_todo(#N, items[])`：把入口整理成工作项。session 认领时缺省自己当负责人；
    /// 调度层 / 人代为派活时每项都要写明负责人。
    pub fn claim_entry(
        &mut self,
        by: &Actor,
        now: Ts,
        entry: EntryNo,
        items: Vec<NewItem>,
    ) -> Result<Applied<Vec<WorkId>>, Rejected> {
        self.transact(by, now, |d| {
            d.permit(Action::ClaimEntry, Ctx::default())?;
            let e =
                d.s.entries
                    .get(&entry)
                    .ok_or(Rejected::UnknownEntry(entry))?;
            if e.closed.is_some() {
                return Err(Rejected::EntryClosed(entry));
            }
            if items.is_empty() {
                return Err(Rejected::Invalid("a claim needs at least one work item"));
            }
            let me = d.by.session().cloned();
            let ids = d.insert_batch(items, None, Some(entry), me.as_ref())?;
            if ids.iter().any(|w| d.s.items[w].owner.is_none()) {
                return Err(Rejected::OwnerRequired);
            }
            d.emit(EventKind::EntryClaimed {
                entry,
                items: ids.clone(),
            });
            Ok(ids)
        })
    }

    /// `work_item_create` / `request_help`：建一项。不写负责人 = 待认领，交调度层派。
    pub fn create_item(
        &mut self,
        by: &Actor,
        now: Ts,
        item: NewItem,
    ) -> Result<Applied<WorkId>, Rejected> {
        self.transact(by, now, |d| {
            d.permit(Action::CreateItem, Ctx::default())?;
            let ids = d.insert_batch(vec![item], None, None, None)?;
            Ok(ids[0])
        })
    }

    /// 把一个群关联到工作项（spec §17：工作项不自动建群，负责人自己建、自己关联）；`None` = 解除。
    pub fn link_channel(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
        channel: Option<ChannelId>,
    ) -> Result<Applied<()>, Rejected> {
        self.transact(by, now, |d| {
            d.permit_on(Action::LinkChannel, id)?;
            d.item_mut(id)?.channel = channel.clone();
            d.emit(EventKind::ChannelLinked { id, channel });
            Ok(())
        })
    }
}

impl Draft {
    pub(crate) fn emit(&mut self, kind: EventKind) {
        self.events.push(kind);
    }

    pub(crate) fn permit(&self, action: Action, ctx: Ctx<'_>) -> Result<(), Rejected> {
        if perm::allowed(&self.by, action, ctx) {
            Ok(())
        } else {
            Err(Rejected::Forbidden(action))
        }
    }

    pub(crate) fn item(&self, id: WorkId) -> Result<&WorkItem, Rejected> {
        self.s.items.get(&id).ok_or(Rejected::UnknownItem(id))
    }

    pub(crate) fn item_mut(&mut self, id: WorkId) -> Result<&mut WorkItem, Rejected> {
        self.s.items.get_mut(&id).ok_or(Rejected::UnknownItem(id))
    }

    /// 取一项，必须还活着（不是 done / dropped）。
    pub(crate) fn live(&self, id: WorkId) -> Result<&WorkItem, Rejected> {
        let it = self.item(id)?;
        if it.status.is_terminal() {
            return Err(Rejected::Terminal(id));
        }
        Ok(it)
    }

    /// 对一项做 `action` 的权限：上下文取该项当前的负责人与待接受的转交对象。
    pub(crate) fn permit_on(&self, action: Action, id: WorkId) -> Result<(), Rejected> {
        let it = self.live(id)?;
        self.permit(
            action,
            Ctx {
                owner: it.owner.as_ref(),
                transfer_to: it.pending_transfer.as_ref().map(|t| &t.to),
                assignee: None,
            },
        )
    }

    /// 按批建项（认领、建项、拆分共用）。`fixed_parent` = 拆分时的原项；`extra_source` = 认领的入口；
    /// `default_owner` = 没写负责人时用谁。
    pub(crate) fn insert_batch(
        &mut self,
        batch: Vec<NewItem>,
        fixed_parent: Option<WorkId>,
        extra_source: Option<EntryNo>,
        default_owner: Option<&SessionId>,
    ) -> Result<Vec<WorkId>, Rejected> {
        let mut ids: Vec<WorkId> = Vec::with_capacity(batch.len());
        for n in batch {
            let title = n.title.trim().to_string();
            if title.is_empty() {
                return Err(Rejected::Blank("title"));
            }
            let parent = match (fixed_parent, n.parent) {
                (Some(_), Some(_)) => {
                    return Err(Rejected::Invalid(
                        "split items always hang under the blocked item",
                    ));
                }
                (Some(p), None) => Some(p),
                (None, Some(r)) => Some(self.resolve(r, &ids)?),
                (None, None) => None,
            };
            if let Some(p) = parent
                && fixed_parent.is_none()
                && !ids.contains(&p)
            {
                let owner = self.item(p)?.owner.clone();
                self.permit(
                    Action::CreateChild,
                    Ctx {
                        owner: owner.as_ref(),
                        ..Ctx::default()
                    },
                )?;
            }
            let mut depends_on = Vec::with_capacity(n.depends_on.len());
            for r in n.depends_on {
                depends_on.push(self.resolve(r, &ids)?);
            }
            depends_on.sort();
            depends_on.dedup();
            let mut sources = n.sources;
            sources.extend(extra_source);
            sources.sort();
            sources.dedup();
            if let Some(missing) = sources.iter().find(|e| !self.s.entries.contains_key(e)) {
                return Err(Rejected::UnknownEntry(*missing));
            }
            let owner = n.owner.or_else(|| default_owner.cloned());
            if let Some(o) = &owner
                && self.by.session() != Some(o)
            {
                self.permit(
                    Action::Assign,
                    Ctx {
                        assignee: Some(o),
                        ..Ctx::default()
                    },
                )?;
            }
            if n.next_check_at.is_some_and(|t| t < self.now) {
                return Err(Rejected::NextCheckNotFuture);
            }
            self.s.last_item += 1;
            let id = WorkId(self.s.last_item);
            let how = if self.by.session() == owner.as_ref() {
                HowChanged::Claim
            } else {
                HowChanged::Assign
            };
            let owner_history = owner
                .iter()
                .map(|o| OwnerChange {
                    from: None,
                    to: o.clone(),
                    by: self.by.clone(),
                    how,
                    note: String::new(),
                    at: self.now,
                })
                .collect();
            self.s.items.insert(
                id,
                WorkItem {
                    id,
                    title: title.clone(),
                    body: n.body,
                    parent,
                    depends_on: depends_on.clone(),
                    sources: sources.clone(),
                    owner: owner.clone(),
                    owner_history,
                    status: Status::Open,
                    blocked_on: Vec::new(),
                    next_check_at: n.next_check_at,
                    missed_checks: 0,
                    evidence: None,
                    drop_reason: None,
                    pending_transfer: None,
                    channel: n.channel,
                    last_report: None,
                    revision: 1,
                    created_at: self.now,
                    updated_at: self.now,
                },
            );
            self.emit(EventKind::ItemCreated {
                id,
                title,
                parent,
                depends_on,
                sources,
                owner,
            });
            ids.push(id);
        }
        Ok(ids)
    }

    fn resolve(&self, r: ItemRef, batch: &[WorkId]) -> Result<WorkId, Rejected> {
        match r {
            ItemRef::Id(w) if self.s.items.contains_key(&w) => Ok(w),
            ItemRef::Id(w) => Err(Rejected::UnknownItem(w)),
            ItemRef::Batch(j) => batch.get(j).copied().ok_or(Rejected::BatchRef(j)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::HOUR;
    use crate::model::{EntryClosure, Role};
    use crate::testkit::*;

    #[test]
    fn registering_an_entry_is_idempotent() {
        let mut b = WorkBook::new(CrewId::new("c"), Config::default());
        let first = b.register_entry(T0, EntryNo(1)).unwrap();
        assert!(first.value);
        assert_eq!(
            first.events,
            [Event {
                seq: 1,
                at: T0,
                by: Actor::System,
                kind: EventKind::EntryRegistered { entry: EntryNo(1) }
            }]
        );
        let again = b.register_entry(T0.plus(5), EntryNo(1)).unwrap();
        assert!(!again.value);
        assert!(again.events.is_empty());
        assert_eq!(b.entry(EntryNo(1)).unwrap().registered_at, T0);
        assert_eq!(b.log().len(), 1);
    }

    #[test]
    fn a_session_claim_makes_it_the_owner_of_every_item() {
        let mut b = book();
        let got = b
            .claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![
                    NewItem::titled("  schema  "),
                    NewItem::titled("ui").after(ItemRef::Batch(0)),
                    NewItem::titled("ui tests").under(ItemRef::Batch(1)),
                ],
            )
            .unwrap();
        assert_eq!(got.value, [WorkId(1), WorkId(2), WorkId(3)]);
        let w1 = b.item(WorkId(1)).unwrap();
        assert_eq!(w1.title, "schema", "标题去掉首尾空白");
        assert_eq!(w1.owner, Some(sid("a")));
        assert_eq!(w1.status, Status::Open);
        assert_eq!(w1.sources, [EntryNo(1)]);
        assert_eq!((w1.revision, w1.created_at, w1.updated_at), (1, T1, T1));
        assert_eq!(
            w1.owner_history,
            [OwnerChange {
                from: None,
                to: sid("a"),
                by: Actor::worker("a"),
                how: HowChanged::Claim,
                note: String::new(),
                at: T1
            }]
        );
        assert_eq!(b.item(WorkId(2)).unwrap().depends_on, [WorkId(1)]);
        assert_eq!(b.item(WorkId(3)).unwrap().parent, Some(WorkId(2)));
        assert_eq!(
            kinds(&got.events),
            [
                "item_created",
                "item_created",
                "item_created",
                "entry_claimed"
            ]
        );
        let seqs: Vec<u64> = got.events.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, [2, 3, 4, 5], "接着登记入口那条（seq 1）往下编");
        assert_eq!(
            got.events[3].kind,
            EventKind::EntryClaimed {
                entry: EntryNo(1),
                items: vec![WorkId(1), WorkId(2), WorkId(3)]
            }
        );
    }

    #[test]
    fn forward_batch_reference_is_rejected_and_consumes_no_number() {
        let mut b = book();
        let before = b.state().clone();
        let err = b
            .claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![
                    NewItem::titled("x").after(ItemRef::Batch(1)),
                    NewItem::titled("y"),
                ],
            )
            .unwrap_err();
        assert_eq!(err, Rejected::BatchRef(1));
        assert_eq!(b.state(), &before, "被拒的命令不动账本");
        let ok = b
            .claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![NewItem::titled("y")],
            )
            .unwrap();
        assert_eq!(ok.value, [WorkId(1)], "W 号没被浪费");
    }

    #[test]
    fn claim_needs_a_known_open_entry_and_at_least_one_item() {
        let mut b = book();
        assert_eq!(
            b.claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(9),
                vec![NewItem::titled("x")]
            ),
            Err(Rejected::UnknownEntry(EntryNo(9)))
        );
        assert_eq!(
            b.claim_entry(&Actor::worker("a"), T1, EntryNo(1), vec![]),
            Err(Rejected::Invalid("a claim needs at least one work item"))
        );
        assert_eq!(
            b.claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![NewItem::titled(" ")]
            ),
            Err(Rejected::Blank("title"))
        );
        let mut state = b.state().clone();
        state.entries.get_mut(&EntryNo(1)).unwrap().closed = Some(EntryClosure {
            by: Actor::Human,
            note: "不做了".into(),
            evidence: None,
            at: T0,
        });
        let mut closed = WorkBook::restore(state, Config::default(), T1).unwrap();
        assert_eq!(
            closed.claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![NewItem::titled("x")]
            ),
            Err(Rejected::EntryClosed(EntryNo(1)))
        );
    }

    #[test]
    fn dispatch_on_behalf_must_name_owners() {
        let mut b = book();
        assert_eq!(
            b.claim_entry(&Actor::System, T1, EntryNo(1), vec![NewItem::titled("x")]),
            Err(Rejected::OwnerRequired)
        );
        let got = b
            .claim_entry(
                &Actor::System,
                T1,
                EntryNo(1),
                vec![NewItem::titled("x").owned_by("a").check_at(T1)],
            )
            .unwrap();
        let w = b.item(got.value[0]).unwrap();
        assert_eq!(w.owner, Some(sid("a")));
        assert_eq!(w.next_check_at, Some(T1), "派活占位项可以「现在就查」");
        assert_eq!(w.owner_history[0].how, HowChanged::Assign);
        assert_eq!(w.owner_history[0].by, Actor::System);
    }

    #[test]
    fn workers_cannot_hand_work_to_others_and_admins_cannot_take_work() {
        let mut b = book();
        assert_eq!(
            b.claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![NewItem::titled("x").owned_by("b")]
            ),
            Err(Rejected::Forbidden(Action::Assign))
        );
        assert_eq!(
            b.claim_entry(
                &Actor::admin("d"),
                T1,
                EntryNo(1),
                vec![NewItem::titled("x")]
            ),
            Err(Rejected::Forbidden(Action::ClaimEntry))
        );
        let ok = b
            .claim_entry(
                &Actor::captain("c"),
                T1,
                EntryNo(1),
                vec![NewItem::titled("x").owned_by("b"), NewItem::titled("y")],
            )
            .unwrap();
        assert_eq!(b.item(ok.value[0]).unwrap().owner, Some(sid("b")));
        assert_eq!(
            b.item(ok.value[1]).unwrap().owner,
            Some(sid("c")),
            "机长也能接活（§13 Q9）"
        );
    }

    #[test]
    fn request_help_creates_an_unowned_item() {
        let mut b = book();
        let w = b
            .create_item(&Actor::worker("a"), T1, NewItem::titled("帮我查日志"))
            .unwrap();
        let it = b.item(w.value).unwrap();
        assert_eq!((it.owner.clone(), it.status), (None, Status::Open));
        assert!(it.owner_history.is_empty());
        assert_eq!(
            b.create_item(
                &Actor::worker("a"),
                T1,
                NewItem::titled("x").after(ItemRef::Batch(0))
            ),
            Err(Rejected::BatchRef(0))
        );
        assert_eq!(
            b.create_item(
                &Actor::worker("a"),
                T1,
                NewItem::titled("x").from_entry(EntryNo(4))
            ),
            Err(Rejected::UnknownEntry(EntryNo(4)))
        );
    }

    #[test]
    fn sub_items_need_the_parent_owner_or_the_captain() {
        let mut b = book();
        let parent = claim(&mut b, "a", &["parent"])[0];
        let child = NewItem::titled("child").under(ItemRef::Id(parent));
        assert_eq!(
            b.create_item(&Actor::worker("b"), T1, child.clone()),
            Err(Rejected::Forbidden(Action::CreateChild))
        );
        assert!(
            b.create_item(&Actor::worker("a"), T1, child.clone())
                .is_ok()
        );
        assert!(b.create_item(&Actor::captain("c"), T1, child).is_ok());
        assert_eq!(
            b.create_item(
                &Actor::worker("a"),
                T1,
                NewItem::titled("x").under(ItemRef::Id(WorkId(99)))
            ),
            Err(Rejected::UnknownItem(WorkId(99)))
        );
    }

    #[test]
    fn child_depending_on_its_parent_is_a_cycle() {
        let mut b = book();
        let parent = claim(&mut b, "a", &["parent"])[0];
        let err = b
            .create_item(
                &Actor::worker("a"),
                T1,
                NewItem::titled("child")
                    .under(ItemRef::Id(parent))
                    .after(ItemRef::Id(parent)),
            )
            .unwrap_err();
        assert_eq!(
            err,
            Rejected::Invariant(Violation::Cycle(vec![parent, WorkId(2)]))
        );
    }

    #[test]
    fn creation_check_times_are_validated() {
        let mut b = book();
        assert_eq!(
            b.create_item(
                &Actor::worker("a"),
                T1,
                NewItem::titled("x").owned_by("a").check_at(T0)
            ),
            Err(Rejected::NextCheckNotFuture)
        );
        assert_eq!(
            b.create_item(
                &Actor::worker("a"),
                T1,
                NewItem::titled("x")
                    .owned_by("a")
                    .check_at(T1.plus(3 * HOUR))
            ),
            Err(Rejected::Invariant(Violation::CheckTooFar {
                item: WorkId(1),
                at: T1.plus(3 * HOUR),
                limit: T1.plus(2 * HOUR)
            }))
        );
        assert_eq!(
            b.create_item(&Actor::worker("a"), T1, NewItem::titled("x").check_at(T1)),
            Err(Rejected::Invariant(Violation::CheckNotAllowed(WorkId(1)))),
            "没负责人的项不排检查"
        );
    }

    #[test]
    fn restore_validates_and_numbering_continues() {
        let mut b = book();
        claim(&mut b, "a", &["x", "y"]);
        let json = serde_json::to_string(b.state()).unwrap();
        let state: BookState = serde_json::from_str(&json).unwrap();
        assert_eq!(&state, b.state(), "JSON 往返不丢字段");
        let mut back = WorkBook::restore(state.clone(), Config::default(), T1).unwrap();
        let next = back
            .create_item(&Actor::worker("a"), T1, NewItem::titled("z"))
            .unwrap();
        assert_eq!(next.value, WorkId(3));
        assert_eq!(
            next.events[0].seq,
            b.state().last_seq + 1,
            "事件序号也接着编"
        );

        let mut broken = state;
        broken.items.get_mut(&WorkId(1)).unwrap().status = Status::Active;
        assert_eq!(
            WorkBook::restore(broken, Config::default(), T1).unwrap_err(),
            Violation::ActiveWithoutCheck(WorkId(1))
        );
    }

    #[test]
    fn channel_link_bumps_revision_and_is_owner_only() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        let ch = Some(ChannelId::new("ch-release"));
        assert_eq!(
            b.link_channel(&Actor::worker("b"), T1, w, ch.clone()),
            Err(Rejected::Forbidden(Action::LinkChannel))
        );
        let later = T1.plus(1_000);
        let got = b
            .link_channel(&Actor::worker("a"), later, w, ch.clone())
            .unwrap();
        assert_eq!(
            got.events[0].kind,
            EventKind::ChannelLinked {
                id: w,
                channel: ch.clone()
            }
        );
        let it = b.item(w).unwrap();
        assert_eq!(
            (it.channel.clone(), it.revision, it.updated_at),
            (ch, 2, later)
        );
        assert_eq!(it.created_at, T1);
        assert_eq!(
            b.link_channel(&Actor::worker("a"), later, WorkId(9), None),
            Err(Rejected::UnknownItem(WorkId(9)))
        );
    }

    #[test]
    fn clock_going_backwards_does_not_wedge_the_book() {
        let mut b = book();
        let far = T1.plus(2 * HOUR);
        b.create_item(
            &Actor::worker("a"),
            T1,
            NewItem::titled("x").owned_by("a").check_at(far),
        )
        .unwrap();
        // 墙钟往回跳了一小时：按传入的 now 算，上面那项的检查时间就「超过上限」了，每条命令都会被拒。
        let back = T1.plus(-HOUR);
        let got = b
            .create_item(&Actor::worker("a"), back, NewItem::titled("y"))
            .unwrap();
        assert_eq!(got.events[0].at, T1, "按账本见过的最晚时间算");
        assert_eq!(b.item(got.value).unwrap().created_at, T1);
        assert_eq!(b.state().clock, T1);
        assert!(WorkBook::restore(b.state().clone(), Config::default(), back).is_ok());
    }

    #[test]
    fn take_log_drains() {
        let mut b = book();
        claim(&mut b, "a", &["x"]);
        let drained = b.take_log();
        assert_eq!(drained.len(), 3);
        assert!(b.log().is_empty());
        assert_eq!(b.state().last_seq, 3);
    }

    #[test]
    fn roles_are_supplied_by_the_caller() {
        // 同一个 session 以 worker 身份不能替别人派活，以机长身份可以：角色由 crewd 认定后传入。
        let mut b = book();
        let as_worker = Actor::Session {
            id: sid("c"),
            role: Role::Worker,
        };
        assert!(
            b.claim_entry(
                &as_worker,
                T1,
                EntryNo(1),
                vec![NewItem::titled("x").owned_by("b")]
            )
            .is_err()
        );
        assert!(
            b.claim_entry(
                &Actor::captain("c"),
                T1,
                EntryNo(1),
                vec![NewItem::titled("x").owned_by("b")]
            )
            .is_ok()
        );
    }
}
