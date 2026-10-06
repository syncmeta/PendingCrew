//! 性质测试：随机命令序列（确定性生成器，不引第三方 crate）打在一本账上，每一步之后检查
//! ① 成功 → 整份不变量成立、事件序号连续、号码只增、项不消失、终结项不复活、账本时间不倒退；
//! ② 被拒 → 账本与事件日志原样不动；
//! ③ 所有只读视图在任何可达状态上都不 panic。
//! 另确认生成器真的让每种命令都成功过、且成功得够多（否则上面的检查是空转）。
use crewd_work::invariants::check;
use crewd_work::*;
use std::collections::{BTreeMap, BTreeSet};

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn chance(&mut self, one_in: u64) -> bool {
        self.below(one_in) == 0
    }
}

const SESSIONS: [&str; 3] = ["a", "b", "c"];

/// 命令种类与抽中的权重。
const KINDS: [(&str, u64); 19] = [
    ("register_entry", 1),
    ("claim_entry", 6),
    ("create_item", 5),
    ("report", 9),
    ("block", 5),
    ("answer_ask", 3),
    ("apply_default", 2),
    ("mark_done", 4),
    ("drop_item", 3),
    ("relink", 2),
    ("request_transfer", 3),
    ("accept_transfer", 3),
    ("withdraw_transfer", 2),
    ("assign", 3),
    ("reassign_lost", 2),
    ("record_checkpoint_wake", 3),
    ("close_entry", 1),
    ("note_escalation", 2),
    ("link_channel", 2),
];

fn pick_kind(r: &mut Rng) -> &'static str {
    let total: u64 = KINDS.iter().map(|(_, w)| w).sum();
    let mut x = r.below(total);
    for (name, w) in KINDS {
        if x < w {
            return name;
        }
        x -= w;
    }
    unreachable!()
}

fn session(r: &mut Rng) -> SessionId {
    SessionId::new(SESSIONS[r.below(3) as usize])
}

fn any_actor(r: &mut Rng) -> Actor {
    match r.below(7) {
        0..=2 => Actor::worker(SESSIONS[r.below(3) as usize]),
        3 => Actor::captain("c"),
        4 => Actor::admin("d"),
        5 => Actor::Human,
        _ => Actor::System,
    }
}

/// 一半概率让操作者就是该项的负责人（否则几乎全被权限拒掉，测不到深处）。
fn actor_for(r: &mut Rng, b: &WorkBook, id: WorkId) -> Actor {
    match b.item(id).and_then(|it| it.owner.clone()) {
        Some(owner) if r.chance(2) => Actor::worker(owner.as_str()),
        _ => any_actor(r),
    }
}

/// 多数时候挑一个满足 `pred` 的项；偶尔挑任意已发出的号或一个还不存在的号。
fn item_where(r: &mut Rng, b: &WorkBook, pred: impl Fn(&WorkItem) -> bool) -> WorkId {
    let hits: Vec<WorkId> = b
        .state()
        .items
        .values()
        .filter(|it| pred(it))
        .map(|it| it.id)
        .collect();
    if hits.is_empty() || r.chance(4) {
        WorkId(1 + r.below(u64::from(b.state().last_item) + 1) as u32)
    } else {
        hits[r.below(hits.len() as u64) as usize]
    }
}

fn item(r: &mut Rng, b: &WorkBook) -> WorkId {
    item_where(r, b, |it| it.status.is_live())
}

/// 多数时候挑一条还开着的问。
fn ask(r: &mut Rng, b: &WorkBook) -> AskId {
    let open: Vec<AskId> = b
        .state()
        .asks
        .values()
        .filter(|a| a.state == AskState::Open)
        .map(|a| a.id)
        .collect();
    if open.is_empty() || r.chance(4) {
        AskId(1 + r.below(u64::from(b.state().last_ask) + 1) as u32)
    } else {
        open[r.below(open.len() as u64) as usize]
    }
}

fn entry(r: &mut Rng, b: &WorkBook) -> EntryNo {
    EntryNo(1 + r.below(b.state().entries.len() as u64 + 1) as i64)
}

fn text(r: &mut Rng, s: &str) -> String {
    if r.chance(12) { " ".into() } else { s.into() }
}

fn evidence(r: &mut Rng) -> Evidence {
    match r.below(5) {
        0 => Evidence::Commit("abc".into()),
        1 => Evidence::Url("https://example.invalid".into()),
        2 | 3 => Evidence::Text("ok".into()),
        _ => Evidence::Text("".into()),
    }
}

/// 下次检查时间：多数合法，偶尔过去 / 太远 / 不给。
fn check_time(r: &mut Rng, now: Ts) -> Option<Ts> {
    match r.below(6) {
        0 => None,
        1 => Some(now.plus(-(r.below(HOUR as u64) as i64))),
        2 => Some(now.plus(2 * HOUR + 1 + r.below(HOUR as u64) as i64)),
        _ => Some(now.plus(1 + r.below(2 * HOUR as u64) as i64)),
    }
}

fn new_item(r: &mut Rng, b: &WorkBook, index: usize) -> NewItem {
    let mut n = NewItem::titled(&text(r, "task"));
    if r.chance(4) {
        n.parent = Some(if index > 0 && r.chance(2) {
            ItemRef::Batch(r.below(index as u64 + 1) as usize)
        } else {
            ItemRef::Id(item(r, b))
        });
    }
    if r.chance(3) {
        n.depends_on.push(if index > 0 && r.chance(2) {
            ItemRef::Batch(r.below(index as u64) as usize)
        } else {
            ItemRef::Id(item(r, b))
        });
    }
    if r.chance(3) {
        n.owner = Some(session(r));
    }
    if r.chance(4) {
        n.next_check_at = check_time(r, b.state().clock);
    }
    n
}

fn batch(r: &mut Rng, b: &WorkBook, max: u64) -> Vec<NewItem> {
    (0..r.below(max + 1) as usize)
        .map(|i| new_item(r, b, i))
        .collect()
}

fn ok<T>(res: Result<Applied<T>, Rejected>) -> Result<Vec<Event>, Rejected> {
    res.map(|a| a.events)
}

/// 随机挑一条命令打上去，返回命令名与结果。
fn step(r: &mut Rng, b: &mut WorkBook, now: Ts) -> (&'static str, Result<Vec<Event>, Rejected>) {
    let kind = pick_kind(r);
    let res = match kind {
        "register_entry" => {
            let no = EntryNo(b.state().entries.len() as i64 + 1);
            ok(b.register_entry(now, no))
        }
        "claim_entry" => {
            let (by, e) = (any_actor(r), entry(r, b));
            let items = batch(r, b, 3);
            ok(b.claim_entry(&by, now, e, items))
        }
        "create_item" => {
            let by = any_actor(r);
            let n = new_item(r, b, 0);
            ok(b.create_item(&by, now, n))
        }
        "report" => {
            let id = item(r, b);
            let by = actor_for(r, b, id);
            let status = match r.below(3) {
                0 => None,
                1 => Some(Progress::Open),
                _ => Some(Progress::Active),
            };
            let report = Report {
                status,
                next_check_at: check_time(r, now),
                note: text(r, "progress"),
            };
            ok(b.report(&by, now, id, report))
        }
        "block" => {
            let id = item(r, b);
            let by = actor_for(r, b, id);
            let mut on = Vec::new();
            for _ in 0..r.below(3) {
                on.push(match r.below(3) {
                    0 => BlockOn::NewAsk {
                        question: text(r, "q?"),
                        default: r.chance(2).then(|| DefaultOption {
                            text: "A".into(),
                            after_ms: r.below(4 * HOUR as u64) as i64,
                        }),
                    },
                    1 => BlockOn::Ask(ask(r, b)),
                    _ => BlockOn::Item(item(r, b)),
                });
            }
            let rest = if r.chance(2) {
                Continuation::Nothing
            } else {
                Continuation::Split(batch(r, b, 2))
            };
            ok(b.block(&by, now, id, on, rest))
        }
        "answer_ask" => {
            let by = if r.chance(2) {
                Actor::Human
            } else {
                any_actor(r)
            };
            let h = ask(r, b);
            ok(b.answer_ask(&by, now, h, &text(r, "yes")))
        }
        "apply_default" => {
            let by = if r.chance(2) {
                Actor::System
            } else {
                any_actor(r)
            };
            let h = ask(r, b);
            ok(b.apply_default(&by, now, h))
        }
        "mark_done" => {
            let id = item(r, b);
            let by = actor_for(r, b, id);
            ok(b.mark_done(&by, now, id, evidence(r)))
        }
        "drop_item" => {
            let id = item(r, b);
            let by = actor_for(r, b, id);
            let kind = [
                DropKind::Duplicate,
                DropKind::Superseded,
                DropKind::NotNeeded,
            ][r.below(3) as usize];
            let ev = r.chance(2).then(|| evidence(r));
            let reason = DropReason {
                kind,
                note: text(r, "why"),
                evidence: ev,
            };
            ok(b.drop_item(&by, now, id, reason))
        }
        "relink" => {
            let id = item(r, b);
            let by = actor_for(r, b, id);
            let parent = r.chance(2).then(|| item(r, b));
            let deps = (0..r.below(3)).map(|_| item(r, b)).collect();
            ok(b.relink(&by, now, id, parent, deps))
        }
        "request_transfer" => {
            let id = item(r, b);
            let by = actor_for(r, b, id);
            let to = session(r);
            ok(b.request_transfer(&by, now, id, to, &text(r, "handover")))
        }
        "accept_transfer" => {
            let id = item_where(r, b, |it| it.pending_transfer.is_some());
            let by = match b.item(id).and_then(|it| it.pending_transfer.clone()) {
                Some(t) if r.chance(2) => Actor::worker(t.to.as_str()),
                _ => any_actor(r),
            };
            ok(b.accept_transfer(&by, now, id))
        }
        "withdraw_transfer" => {
            let id = item_where(r, b, |it| it.pending_transfer.is_some());
            let by = actor_for(r, b, id);
            ok(b.withdraw_transfer(&by, now, id))
        }
        "assign" => {
            let (id, by, to) = (item(r, b), any_actor(r), session(r));
            ok(b.assign(&by, now, id, to, &text(r, "assign")))
        }
        "reassign_lost" => {
            let (id, by, to) = (item(r, b), any_actor(r), session(r));
            ok(b.reassign_lost(&by, now, id, to, &text(r, "lost")))
        }
        "record_checkpoint_wake" => {
            let ids: Vec<WorkId> = (0..1 + r.below(2)).map(|_| item(r, b)).collect();
            let by = if r.chance(2) {
                Actor::System
            } else {
                any_actor(r)
            };
            ok(b.record_checkpoint_wake(&by, now, &ids))
        }
        "close_entry" => {
            let (by, e) = (any_actor(r), entry(r, b));
            let ev = r.chance(2).then(|| evidence(r));
            ok(b.close_entry(&by, now, e, &text(r, "ruling"), ev))
        }
        "note_escalation" => {
            let by = if r.chance(2) {
                Actor::System
            } else {
                any_actor(r)
            };
            let tier =
                [WaitTier::Fresh, WaitTier::Pinned, WaitTier::StillWanted][r.below(3) as usize];
            ok(b.note_escalation(&by, now, ask(r, b), tier))
        }
        "link_channel" => {
            let id = item(r, b);
            let by = actor_for(r, b, id);
            let ch = r.chance(2).then(|| ChannelId::new("ch"));
            ok(b.link_channel(&by, now, id, ch))
        }
        other => unreachable!("{other}"),
    };
    (kind, res)
}

/// 所有只读视图都要能在任何可达状态上跑（它们按不变量用 `[]` 取值，坏状态会 panic）。
fn exercise_views(b: &WorkBook, now: Ts) {
    let dead: BTreeSet<SessionId> = [SessionId::new("b")].into();
    let _ = b.entry_views();
    let _ = b.ready_items();
    let _ = b.unowned_items();
    let _ = b.due_checkpoints(now);
    let _ = b.owner_lost(now, &dead);
    let _ = b.human_waits(now);
    let _ = b.escalations_due(now);
    let _ = b.defaults_due(now);
    let _ = b.patrol(now, &dead);
    for s in SESSIONS {
        let s = SessionId::new(s);
        let _ = b.my_items(&s);
        let _ = b.load_of(&s);
        let _ = b.incoming_transfers(&s);
        let _ = b.stop_violations(&s, now);
    }
}

fn run(seed: u64, steps: usize, wins: &mut BTreeMap<&'static str, usize>) {
    let mut r = Rng::new(seed);
    let mut b = WorkBook::new(CrewId::new("crew"), Config::default());
    let mut now = Ts(1_700_000_000_000);
    b.register_entry(now, EntryNo(1)).unwrap();
    for i in 0..steps {
        // 时间大多往前走，偶尔墙钟往回跳。
        now = if r.chance(20) {
            now.plus(-(r.below(20 * MINUTE as u64) as i64))
        } else {
            now.plus(r.below(40 * MINUTE as u64) as i64)
        };
        let before = b.state().clone();
        let log_len = b.log().len();
        let (kind, res) = step(&mut r, &mut b, now);
        let ctx = format!("seed {seed} step {i} {kind}");
        match res {
            Ok(events) => {
                *wins.entry(kind).or_default() += 1;
                let after = b.state();
                assert_eq!(check(after, b.config(), after.clock), Ok(()), "{ctx}");
                let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
                let expect: Vec<u64> = (before.last_seq + 1..=after.last_seq).collect();
                assert_eq!(seqs, expect, "{ctx}: 序号连续");
                assert!(events.iter().all(|e| e.at == after.clock), "{ctx}");
                assert_eq!(b.log().len(), log_len + events.len(), "{ctx}");
                assert!(after.last_item >= before.last_item, "{ctx}");
                assert!(after.last_ask >= before.last_ask, "{ctx}");
                assert!(after.clock >= before.clock, "{ctx}: 账本时间不倒退");
                for (id, old) in &before.items {
                    let new = &after.items[id];
                    assert!(new.revision >= old.revision, "{ctx}");
                    if old.status.is_terminal() {
                        assert_eq!(new, old, "{ctx}: 终结项不再变");
                    }
                }
            }
            Err(_) => {
                assert_eq!(b.state(), &before, "{ctx}: 被拒的命令改了账本");
                assert_eq!(b.log().len(), log_len, "{ctx}");
            }
        }
        if r.chance(5) {
            let s = session(&mut r);
            let _ = b.on_idle(&s, now);
            let st = b.state();
            assert_eq!(check(st, b.config(), st.clock), Ok(()), "{ctx}: on_idle");
        }
        exercise_views(&b, now);
    }
}

#[test]
fn random_command_sequences_preserve_every_invariant() {
    let mut wins = BTreeMap::new();
    for seed in 0..48 {
        run(seed, 400, &mut wins);
    }
    let total: usize = wins.values().sum();
    assert!(
        total > 3_000,
        "成功的命令太少（{total} / 19200），生成器没打到深处：{wins:?}"
    );
}

#[test]
fn every_command_kind_succeeds_somewhere() {
    let mut wins = BTreeMap::new();
    for seed in 100..148 {
        run(seed, 400, &mut wins);
    }
    let never: Vec<&str> = KINDS
        .iter()
        .map(|(k, _)| *k)
        .filter(|k| wins.get(k).copied().unwrap_or(0) < 5)
        .collect();
    assert_eq!(never, Vec::<&str>::new(), "这些命令成功不到 5 次：{wins:?}");
}
