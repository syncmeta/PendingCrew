//! codex 回合跟踪与结构化记录（Swift `CodexAppServerBackend.trackTurn` / `CodexTranscript`
//! 的移植）。纯计算：喂通知，吐「发生了什么」，不碰 IO、不看钟。
//!
//! - 回合：`turn/started` 置当前回合并开始干活；属于当前回合（或没带回合 id）的 `item/*` 也算在干活
//!   （补丢了的开始边沿；已完成回合的迟到条目不算）；
//!   **id 对得上**的 `turn/completed` 才结束（旧回合迟到的完成不能清掉新回合）。
//! - 回合正常结束且不是压缩回合 → 报「回合结束」，附**本回合**最后一条 agent 正文。
//! - 自动压缩：回合结束时上下文压力够（[`proto::should_compact`]）→ 报「该压缩了」；
//!   token 读数可能比 `turn/completed` 晚到，晚到时再判一次。
//! - 结构化记录：只收 `item/completed` 的条目原样（有上限），渲染成文本给 `screen_text` 用。
use std::collections::VecDeque;

use serde_json::Value;

use crate::codex::proto::{self, ContextUsage};

/// 喂一条通知之后的结论。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Signals {
    /// Only a genuinely active exact turn, with explicit completed/failed status.
    pub notification_outcome: Option<(String,bool)>,
    /// 一个普通回合正常结束：本回合最后一条 agent 正文（没有就是空串）。
    pub turn_ended: Option<String>,
    /// 该触发自动压缩了（调用方发 `thread/compact/start` 并调 [`TurnTracker::begin_compaction`]）。
    pub compact: bool,
    /// 压缩回合结束了（成功与否见 `compaction_problem`）——排队的正文可以放出去了。
    pub compaction_finished: bool,
    pub compaction_problem: Option<String>,
    /// 一个回合以 `completed` 结束（用来清掉过时的额度 / 失败 health）。
    pub completed_ok: bool,
    /// 一个完成了的条目：`(turn_id, item)`。
    pub item: Option<(String, Value)>,
}

#[derive(Debug)]
pub struct TurnTracker {
    thread_id: String,
    active_turn: Option<String>,
    last_completed: Option<String>,
    working: bool,
    compacting: bool,
    compacted_in_turn: bool,
    last_agent_message: Option<String>,
    turns_since_compaction: u32,
    usage: Option<ContextUsage>,
    turn_start_cumulative: Option<i64>,
    /// 回合结束时 token 读数还没到：(turn_id, 开始时的累计, 当时的「距上次压缩」)。
    usage_pending_for: Option<(String, Option<i64>, u32)>,
}

impl TurnTracker {
    pub fn new(thread_id: impl Into<String>) -> Self {
        Self {
            thread_id: thread_id.into(),
            active_turn: None,
            last_completed: None,
            working: false,
            compacting: false,
            compacted_in_turn: false,
            last_agent_message: None,
            // 新会话允许第一次就压（Swift 从 3 起）。
            turns_since_compaction: 3,
            usage: None,
            turn_start_cumulative: None,
            usage_pending_for: None,
        }
    }

    /// 在干活（UI 的「干活中」与 `Working` 边沿）。
    pub fn is_working(&self) -> bool {
        self.working
    }

    /// 不能开新回合：有进行中的回合或正在压缩。
    pub fn is_busy(&self) -> bool {
        self.active_turn.is_some() || self.compacting
    }

    pub fn is_compacting(&self) -> bool {
        self.compacting
    }

    pub fn active_turn(&self) -> Option<&str> {
        self.active_turn.as_deref()
    }

    pub fn usage(&self) -> Option<&ContextUsage> {
        self.usage.as_ref()
    }

    /// `turn/start` 的回包带着回合 id：没有进行中的回合、且它不是刚结束的那个，就记成当前回合
    /// （`turn/started` 可能晚到；完成通知若先到了，不能再把它记回去，否则永远「在忙」）。
    pub fn on_turn_accepted(&mut self, turn_id: &str) {
        if self.active_turn.is_none() && self.last_completed.as_deref() != Some(turn_id) {
            self.active_turn = Some(turn_id.to_string());
            self.working = true;
        }
    }

    /// 我们发了 `thread/compact/start`（自动或手动）：进入压缩态。
    pub fn begin_compaction(&mut self) {
        self.compacting = true;
        self.working = true;
        self.usage_pending_for = None;
    }

    /// `thread/compact/start` 被拒 / 超时：退出压缩态（不再自动重试，直到再过两个回合）。
    pub fn compaction_failed(&mut self) {
        self.compacting = false;
        self.turns_since_compaction = 0;
        self.working = self.active_turn.is_some();
    }

    /// 进程没了：一切归零。
    pub fn reset(&mut self) {
        self.active_turn = None;
        self.working = false;
        self.compacting = false;
        self.usage_pending_for = None;
    }

    pub fn on_notification(&mut self, method: &str, params: &Value) -> Signals {
        let mut s = Signals::default();
        match method {
            "thread/tokenUsage/updated" => self.on_usage(params, &mut s),
            "turn/started" => {
                self.active_turn = params
                    .pointer("/turn/id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.turn_start_cumulative = self.usage.as_ref().map(|u| u.cumulative_tokens);
                self.usage_pending_for = None;
                self.compacted_in_turn = false;
                self.last_agent_message = None;
                self.working = true;
            }
            "turn/completed" => self.on_completed(params, &mut s),
            "item/started" => {
                self.note_item_activity(params);
                if let Some(item) = params.get("item") {
                    let turn = params
                        .get("turnId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    s.item = Some((turn, item.clone()));
                }
            }
            "item/completed" => {
                self.note_item_activity(params);
                if let Some(item) = params.get("item") {
                    match item.get("type").and_then(Value::as_str) {
                        Some("contextCompaction") => {
                            self.compacted_in_turn = true;
                            self.turns_since_compaction = 0;
                        }
                        Some("agentMessage") if self.is_foreign_turn(params) => {}
                        Some("agentMessage") => {
                            self.last_agent_message = Some(
                                item.get("text")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                            );
                        }
                        _ => {}
                    }
                    let turn = params
                        .get("turnId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    s.item = Some((turn, item.clone()));
                }
            }
            m if m.starts_with("item/") => self.note_item_activity(params),
            _ => {}
        }
        s
    }

    /// 条目通知算「在干活」，但只认属于进行中回合的条目：带着回合 id 的，必须就是当前回合
    /// （完成之后才到的旧条目不能把「干活中」重新立起来、永远等不到收尾）；没带回合 id 的
    /// 仍当作干活的证据，补丢了的开始边沿。
    fn note_item_activity(&mut self, params: &Value) {
        match params.get("turnId").and_then(Value::as_str) {
            Some(turn) if self.active_turn.as_deref() != Some(turn) => {}
            _ => self.working = true,
        }
    }

    /// 通知带着回合 id、而且不是当前进行中的回合：别的回合迟到的条目。
    fn is_foreign_turn(&self, params: &Value) -> bool {
        match (
            params.get("turnId").and_then(Value::as_str),
            self.active_turn.as_deref(),
        ) {
            (Some(turn), Some(active)) => turn != active,
            _ => false,
        }
    }

    fn on_usage(&mut self, params: &Value, s: &mut Signals) {
        let Some(usage) = proto::context_usage(params, &self.thread_id) else {
            return;
        };
        if self.turn_start_cumulative.is_none() {
            self.turn_start_cumulative = Some(usage.cumulative_tokens);
        }
        self.usage = Some(usage.clone());
        if let Some((turn, baseline, since)) = self.usage_pending_for.clone()
            && turn == usage.turn_id
            && self.active_turn.is_none()
            && !self.compacting
        {
            self.usage_pending_for = None;
            s.compact = proto::should_compact(&usage, spent(&usage, baseline), since);
        }
    }

    fn on_completed(&mut self, params: &Value, s: &mut Signals) {
        let completed_id = params
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let (Some(active), Some(done)) = (&self.active_turn, &completed_id)
            && active != done
        {
            return;
        }
        // 刚结束的那个回合又来一次完成（重复 / 陈旧）：不是第二次回答，也不能提前结束压缩。
        if completed_id.is_some()
            && completed_id == self.last_completed
            && self.active_turn != completed_id
        {
            return;
        }
        let ok = params.pointer("/turn/status").and_then(Value::as_str) == Some("completed");
        let finishing_compaction = self.compacting;
        let failed=params.pointer("/turn/status").and_then(Value::as_str)==Some("failed");
        if !finishing_compaction && (ok||failed) && self.active_turn.is_some() && self.active_turn==completed_id {
            s.notification_outcome=completed_id.clone().map(|id|(id,failed));
        }

        if finishing_compaction {
            s.compaction_finished = true;
            if !ok {
                let why = params
                    .pointer("/turn/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("回合未完成");
                s.compaction_problem = Some(format!("Codex 压缩失败：{why}"));
            }
        }
        if ok && !finishing_compaction {
            // 压缩回合没有新回答；回合里压过一次又没有新回答的，也不算一次回答（别重复发群）。
            if !self.compacted_in_turn || self.last_agent_message.is_some() {
                s.turn_ended = Some(self.last_agent_message.clone().unwrap_or_default());
            }
            if !self.compacted_in_turn
                && let Some(done) = &completed_id
            {
                match &self.usage {
                    Some(u) if &u.turn_id == done => {
                        s.compact = proto::should_compact(
                            u,
                            spent(u, self.turn_start_cumulative),
                            self.turns_since_compaction,
                        );
                    }
                    _ => {
                        self.usage_pending_for = Some((
                            done.clone(),
                            self.turn_start_cumulative,
                            self.turns_since_compaction,
                        ));
                    }
                }
            }
        }
        s.completed_ok = ok;
        self.last_completed = completed_id;
        self.active_turn = None;
        self.last_agent_message = None;
        if finishing_compaction || self.compacted_in_turn {
            self.turns_since_compaction = 0;
        } else {
            self.turns_since_compaction = (self.turns_since_compaction + 1).min(3);
        }
        if finishing_compaction {
            self.compacting = false;
        }
        self.working = false;
    }
}

fn spent(usage: &ContextUsage, baseline: Option<i64>) -> i64 {
    usage.cumulative_tokens - baseline.unwrap_or(usage.cumulative_tokens)
}

/// 最近完成的条目（`item/completed` 原样），有上限。
#[derive(Debug)]
pub struct Transcript {
    items: VecDeque<(String, Value)>,
    cap: usize,
}

impl Transcript {
    pub fn new(cap: usize) -> Self {
        Self {
            items: VecDeque::new(),
            cap: cap.max(1),
        }
    }

    pub fn push(&mut self, turn_id: String, item: Value) {
        if self.items.len() == self.cap {
            self.items.pop_front();
        }
        self.items.push_back((turn_id, item));
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 渲染成文本，取最后 `max_lines` 行（`screen_text` 的 codex 版）。
    pub fn text(&self, max_lines: usize) -> String {
        let lines: Vec<String> = self
            .items
            .iter()
            .flat_map(|(_, i)| render_item(i))
            .collect();
        let start = lines.len().saturating_sub(max_lines);
        lines[start..].join("\n")
    }
}

/// 一个条目的文本形状。认不出的类型给一行 `[类型]`（codex 加了新条目也不至于看不见）。
pub fn render_item(item: &Value) -> Vec<String> {
    let s = |k: &str| item.get(k).and_then(Value::as_str).unwrap_or_default();
    let block = |prefix: &str, text: &str| -> Vec<String> {
        text.lines().map(|l| format!("{prefix}{l}")).collect()
    };
    match s("type") {
        "userMessage" => {
            let text: Vec<&str> = item
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|c| c.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|c| c.get("text").and_then(Value::as_str))
                .collect();
            block("› ", &text.join("\n"))
        }
        "agentMessage" => block("", s("text")),
        "reasoning" => {
            let summary: Vec<&str> = item
                .get("summary")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            block("· ", &summary.join("\n"))
        }
        "commandExecution" => {
            let code = item
                .get("exitCode")
                .and_then(Value::as_i64)
                .map(|c| format!("（exit {c}）"))
                .unwrap_or_default();
            vec![format!("$ {}{code}", s("command"))]
        }
        "fileChange" => {
            let paths: Vec<&str> = item
                .get("changes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|c| c.get("path").and_then(Value::as_str))
                .collect();
            vec![format!("✎ {}", paths.join(", "))]
        }
        "mcpToolCall" => vec![format!(
            "⚙ {}.{}（{}）",
            s("server"),
            s("tool"),
            s("status")
        )],
        "contextCompaction" => vec!["（上下文已压缩）".to_string()],
        "" => vec!["[?]".to_string()],
        other => vec![format!("[{other}]")],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn started(id: &str) -> Value {
        json!({ "threadId": "thr", "turn": { "id": id, "status": "inProgress", "items": [] } })
    }

    fn completed(id: &str, status: &str) -> Value {
        json!({ "threadId": "thr", "turn": { "id": id, "status": status, "items": [] } })
    }

    fn agent(turn: &str, text: &str) -> Value {
        json!({ "threadId": "thr", "turnId": turn, "completedAtMs": 0,
                "item": { "id": format!("i-{text}"), "type": "agentMessage", "text": text } })
    }

    fn usage(turn: &str, context: i64, total: i64) -> Value {
        json!({ "threadId": "thr", "turnId": turn, "tokenUsage": {
            "last": { "totalTokens": context }, "total": { "totalTokens": total },
            "modelContextWindow": 258_400 } })
    }

    #[test]
    fn a_turn_starts_works_and_ends_with_its_own_last_answer() {
        let mut t = TurnTracker::new("thr");
        t.on_notification("turn/started", &started("t1"));
        assert!(t.is_busy() && t.is_working());
        t.on_notification("item/completed", &agent("t1", "第一句"));
        let s = t.on_notification("item/completed", &agent("t1", "最后一句"));
        assert_eq!(s.item.as_ref().unwrap().0, "t1");
        let s = t.on_notification("turn/completed", &completed("t1", "completed"));
        assert_eq!(s.turn_ended.as_deref(), Some("最后一句"));
        assert!(s.completed_ok);
        assert!(!t.is_busy() && !t.is_working());
        // 下一回合没有 agent 正文：报空串，不拿上一回合的冒充。
        t.on_notification("turn/started", &started("t2"));
        let s = t.on_notification("turn/completed", &completed("t2", "completed"));
        assert_eq!(s.turn_ended.as_deref(), Some(""));
    }

    #[test]
    fn a_late_completion_of_an_old_turn_cannot_end_the_new_one() {
        let mut t = TurnTracker::new("thr");
        t.on_notification("turn/started", &started("new"));
        let s = t.on_notification("turn/completed", &completed("old", "completed"));
        assert_eq!(s, Signals::default());
        assert_eq!(t.active_turn(), Some("new"));
    }

    #[test]
    fn item_activity_repairs_a_missing_start_edge() {
        let mut t = TurnTracker::new("thr");
        t.on_notification("item/started", &json!({ "item": { "type": "reasoning" } }));
        assert!(t.is_working());
        assert!(!t.is_busy(), "没有回合 id 就不算占着回合");
        t.on_notification("turn/completed", &completed("x", "completed"));
        assert!(!t.is_working());
    }

    #[test]
    fn a_late_item_of_a_finished_turn_does_not_make_the_session_look_busy() {
        let mut t = TurnTracker::new("thr");
        t.on_notification("turn/started", &started("t1"));
        t.on_notification("turn/completed", &completed("t1", "completed"));
        assert!(!t.is_working());
        // 完成通知之后才到的 t1 的条目：照样记进结构化记录，但不能把「干活中」立起来。
        let s = t.on_notification("item/completed", &agent("t1", "迟到的话"));
        assert!(s.item.is_some());
        assert!(!t.is_working(), "迟到的条目不属于任何进行中的回合");
        t.on_notification(
            "item/started",
            &json!({ "threadId": "thr", "turnId": "t1", "item": { "type": "reasoning" } }),
        );
        assert!(!t.is_working());
        // 正在跑的回合里别的回合的条目同样不算。
        t.on_notification("turn/started", &started("t2"));
        t.on_notification("turn/completed", &completed("t2", "completed"));
        t.on_notification("turn/started", &started("t3"));
        assert!(t.is_working());
        t.on_notification("turn/completed", &completed("t3", "completed"));
        t.on_notification("item/completed", &agent("t2", "更老的"));
        assert!(!t.is_working());
    }

    #[test]
    fn failed_and_interrupted_turns_end_work_without_a_turn_ended_report() {
        let mut t = TurnTracker::new("thr");
        t.on_notification("turn/started", &started("t1"));
        let s = t.on_notification("turn/completed", &completed("t1", "interrupted"));
        assert_eq!(s.turn_ended, None);
        assert!(!s.completed_ok);
        assert!(!t.is_working());
    }

    #[test]
    fn accepted_turn_id_counts_as_busy_unless_it_already_completed() {
        let mut t = TurnTracker::new("thr");
        t.on_turn_accepted("t1");
        assert_eq!(t.active_turn(), Some("t1"));
        t.on_notification("turn/completed", &completed("t1", "completed"));
        // 回包晚于完成通知到：不能再把这个回合记成进行中。
        t.on_turn_accepted("t1");
        assert!(!t.is_busy());
    }

    #[test]
    fn high_context_turn_asks_for_one_compaction_and_its_completion_is_not_an_answer() {
        let mut t = TurnTracker::new("thr");
        t.on_notification("turn/started", &started("work"));
        t.on_notification("item/completed", &agent("work", "做完了"));
        let s = t.on_notification("turn/completed", &completed("work", "completed"));
        assert_eq!(s.turn_ended.as_deref(), Some("做完了"));
        assert!(!s.compact, "读数还没到");
        // 读数晚于完成到达：这时才判。
        let s = t.on_notification(
            "thread/tokenUsage/updated",
            &usage("work", 190_000, 500_000),
        );
        assert!(s.compact);
        t.begin_compaction();
        assert!(t.is_busy() && t.is_working());
        t.on_notification("turn/started", &started("compact"));
        t.on_notification(
            "item/completed",
            &json!({ "turnId": "compact", "item": { "id": "c", "type": "contextCompaction" } }),
        );
        let s = t.on_notification(
            "thread/tokenUsage/updated",
            &usage("compact", 28_000, 525_000),
        );
        assert!(!s.compact, "压缩中不再触发");
        let s = t.on_notification("turn/completed", &completed("compact", "completed"));
        assert!(s.compaction_finished);
        assert_eq!(s.compaction_problem, None);
        assert_eq!(s.turn_ended, None, "压缩完成不是第二次回答");
        assert!(!t.is_busy() && !t.is_working());
        assert_eq!(t.usage().unwrap().context_tokens, 28_000);
        // 刚压过：下一个高压回合不立刻再压。
        t.on_notification("turn/started", &started("w2"));
        t.on_notification("thread/tokenUsage/updated", &usage("w2", 200_000, 900_000));
        let s = t.on_notification("turn/completed", &completed("w2", "completed"));
        assert!(!s.compact);
    }

    #[test]
    fn usage_already_in_hand_decides_at_completion() {
        let mut t = TurnTracker::new("thr");
        t.on_notification("turn/started", &started("w"));
        t.on_notification("thread/tokenUsage/updated", &usage("w", 190_000, 500_000));
        let s = t.on_notification("turn/completed", &completed("w", "completed"));
        assert!(s.compact);
    }

    #[test]
    fn failed_compaction_turn_reports_the_problem() {
        let mut t = TurnTracker::new("thr");
        t.begin_compaction();
        t.on_notification("turn/started", &started("c"));
        let s = t.on_notification(
            "turn/completed",
            &json!({ "turn": { "id": "c", "status": "failed", "error": { "message": "boom" } } }),
        );
        assert!(s.compaction_finished);
        assert_eq!(
            s.compaction_problem.as_deref(),
            Some("Codex 压缩失败：boom")
        );
        assert!(!t.is_compacting());
    }

    #[test]
    fn compaction_rpc_failure_leaves_the_compacting_state() {
        let mut t = TurnTracker::new("thr");
        t.begin_compaction();
        t.compaction_failed();
        assert!(!t.is_busy() && !t.is_working());
    }

    #[test]
    fn transcript_keeps_the_last_items_and_renders_them() {
        let mut tr = Transcript::new(3);
        tr.push(
            "t".into(),
            json!({ "type": "userMessage", "content": [{ "type": "text", "text": "修 bug" }] }),
        );
        tr.push(
            "t".into(),
            json!({ "type": "commandExecution", "command": "cargo test", "exitCode": 0 }),
        );
        tr.push(
            "t".into(),
            json!({ "type": "fileChange", "changes": [{ "path": "src/a.rs" }] }),
        );
        tr.push(
            "t".into(),
            json!({ "type": "agentMessage", "text": "改好了\n测试全绿" }),
        );
        assert_eq!(tr.len(), 3, "超过上限丢最旧的");
        assert_eq!(
            tr.text(10),
            "$ cargo test（exit 0）\n✎ src/a.rs\n改好了\n测试全绿"
        );
        assert_eq!(tr.text(2), "改好了\n测试全绿");
        assert_eq!(
            render_item(&json!({ "type": "imageView" })),
            vec!["[imageView]"]
        );
        assert_eq!(
            render_item(
                &json!({ "type": "mcpToolCall", "server": "crew", "tool": "post_to_crew", "status": "completed" })
            ),
            vec!["⚙ crew.post_to_crew（completed）"]
        );
    }

    #[test]
    fn another_turns_agent_message_is_not_this_turns_answer() {
        let mut t = TurnTracker::new("thr");
        t.on_notification("turn/started", &started("b"));
        // 旧回合 a 迟到的正文：照常交给记录，但不能当 b 的结论。
        let s = t.on_notification("item/completed", &agent("a", "旧回合的话"));
        assert_eq!(s.item.as_ref().unwrap().0, "a");
        let s = t.on_notification("turn/completed", &completed("b", "completed"));
        assert_eq!(s.turn_ended.as_deref(), Some(""));
    }

    #[test]
    fn a_duplicate_completion_of_the_last_turn_is_ignored() {
        let mut t = TurnTracker::new("thr");
        t.on_notification("turn/started", &started("t1"));
        t.on_notification("item/completed", &agent("t1", "答"));
        let s = t.on_notification("turn/completed", &completed("t1", "completed"));
        assert_eq!(s.turn_ended.as_deref(), Some("答"));
        // 空闲时又来一次同一回合的完成：不是第二次回答。
        let s = t.on_notification("turn/completed", &completed("t1", "completed"));
        assert_eq!(s, Signals::default());
        // 压缩进行中来的陈旧完成：不能提前结束压缩。
        t.begin_compaction();
        let s = t.on_notification("turn/completed", &completed("t1", "completed"));
        assert_eq!(s, Signals::default());
        assert!(t.is_compacting() && t.is_busy());
    }
}
