//! post-tool-use (spec 22.3, "hook behaviour"): unread messages after the cursor, in the channels this
//! session is in or listens to, become `additionalContext`. Visibility, the "sent to X" marks and the
//! rendering are M3a's (`cursors::unread_for`, `render::{blocks_for, injection}`); this file adds the
//! total length cap and decides how far the cursors move (the cursor is shared with wake-ups: at
//! most once per message).
//!
//! The cap (R6): what does not fit is left out from the NEWEST end, so what is injected is a
//! contiguous prefix of each channel's unread run. Cursors move only to the highest seq actually
//! injected; omitted messages stay unread and arrive with the next call. The omission note never
//! marks anything read.
use crewd_collab::render::ChannelBlock;
use crewd_proto::agent::HookResult;
use serde_json::{Value, json};

use crate::AppState;
use crate::agent::Identity;
use crate::agent::port::{self, PortResult};
use crate::agent::text::{self, Locale};
use crate::agent::tools::channel_label_for;

/// The MCP server name in `mcp.json` is `crew` (see crewd-exec `prepare_claude`).
const CREWD_TOOL_PREFIX: &str = "mcp__crew__";

/// crewd's own tools that only talk or read (B14). A session answering another session through
/// `post` is not output: two sessions replying to each other would otherwise keep the loop breaker
/// from ever firing. State-changing crew tools (start / stop / nudge, channel create / invite /
/// archive / transfer, ask, respond_todo, schedule_wakeup, set_session_profile, crew structure
/// tools) are not listed, so they count.
const PASSIVE_CREW_TOOLS: &[&str] = &[
    "post",
    "read_channel",
    "search",
    "list_channels",
    "human_watching",
    "directory",
    "contact",
    "channel_listen",
    "channel_unlisten",
];

pub fn is_crewd_tool(tool_name: &str) -> bool {
    tool_name.starts_with(CREWD_TOOL_PREFIX)
}

/// Does a finished tool call count as output for the loop breaker (C3 + B14)?
pub fn counts_as_progress(tool_name: &str) -> bool {
    match tool_name.strip_prefix(CREWD_TOOL_PREFIX) {
        Some(own) => !PASSIVE_CREW_TOOLS.contains(&own),
        None => true,
    }
}

pub struct Prepared {
    pub context: Option<String>,
    /// Per channel, the highest seq that is really in `context`.
    advance_to: Vec<(String, i64)>,
}

pub async fn prepare(st: &AppState, me: &Identity) -> PortResult<Option<Prepared>> {
    let loc = port::locale(st).await?;
    let cap = st.agent.config.inject_cap_chars;
    let unread = port::unread(st, &me.session_id).await?;
    if unread.iter().all(|(_, v)| v.is_empty()) {
        return Ok(None);
    }
    let blocks = port::blocks(st, &me.session_id, loc, unread).await?;
    let mut labels = Vec::with_capacity(blocks.len());
    for b in &blocks {
        labels.push(
            channel_label_for(
                st,
                &me.session_id,
                &b.channel_id,
                b.kind,
                &b.name,
                &b.crew_title,
                loc,
            )
            .await,
        );
    }
    let fitted = fit(loc, blocks, &labels, cap);
    let advance_to = fitted
        .blocks
        .iter()
        .filter_map(|b| b.lines.last().map(|l| (b.channel_id.clone(), l.seq)))
        .collect();
    Ok(Some(Prepared {
        context: (!fitted.context.trim().is_empty()).then_some(fitted.context),
        advance_to,
    }))
}

struct Fitted {
    /// The blocks as injected (a prefix of every channel's run).
    blocks: Vec<ChannelBlock>,
    context: String,
}

/// The block lines in the order they are dropped: always the newest tail among all channels
/// (`at`, then seq), so every channel keeps a prefix. The last remaining line is never dropped.
fn removal_order(blocks: &[ChannelBlock]) -> Vec<usize> {
    let mut tails: Vec<usize> = blocks.iter().map(|b| b.lines.len()).collect();
    let total: usize = tails.iter().sum();
    let mut order = Vec::new();
    while order.len() + 1 < total {
        let key = |i: usize| {
            let l = &blocks[i].lines[tails[i] - 1];
            (l.at.clone(), l.seq)
        };
        let Some(i) = (0..blocks.len())
            .filter(|i| tails[*i] > 0)
            .max_by_key(|i| key(*i))
        else {
            break;
        };
        tails[i] -= 1;
        order.push(i);
    }
    order
}

fn trimmed(blocks: &[ChannelBlock], order: &[usize]) -> (Vec<ChannelBlock>, Vec<usize>) {
    let mut out: Vec<ChannelBlock> = blocks.to_vec();
    let mut omitted = vec![0usize; blocks.len()];
    for &i in order {
        out[i].lines.pop();
        omitted[i] += 1;
    }
    (out, omitted)
}

fn render(
    loc: Locale,
    blocks: &[ChannelBlock],
    labels: &[String],
    omitted: &[usize],
    cut: Option<(usize, i64)>,
) -> String {
    let body = port::injection(loc, blocks);
    let mut notes: Vec<String> = Vec::new();
    if let Some((bi, seq)) = cut {
        notes.push(text::r(
            loc,
            "inject.truncated",
            &[("channel", &labels[bi]), ("seq", &seq.to_string())],
        ));
    }
    for (bi, n) in omitted.iter().enumerate().filter(|(_, n)| **n > 0) {
        notes.push(text::r(
            loc,
            "inject.omitted",
            &[("channel", &labels[bi]), ("count", &n.to_string())],
        ));
    }
    if notes.is_empty() {
        return body;
    }
    if body.is_empty() {
        return notes.join("\n");
    }
    format!("{body}\n\n{}", notes.join("\n"))
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

/// Smallest `k` in `(lo, hi]` for which `fits(k)`, given that `fits(hi)` and size shrinks as `k` grows.
fn first_fit(mut lo: usize, mut hi: usize, fits: impl Fn(usize) -> bool) -> usize {
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if fits(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

fn fit(loc: Locale, blocks: Vec<ChannelBlock>, labels: &[String], cap: usize) -> Fitted {
    let order = removal_order(&blocks);
    let size_at = |k: usize| {
        let (b, o) = trimmed(&blocks, &order[..k]);
        chars(&render(loc, &b, labels, &o, None))
    };
    let n = order.len();
    let k = if size_at(0) <= cap {
        Some(0)
    } else if size_at(n) <= cap {
        Some(first_fit(0, n, |k| size_at(k) <= cap))
    } else {
        None
    };
    if let Some(k) = k {
        let (blocks, omitted) = trimmed(&blocks, &order[..k]);
        let context = render(loc, &blocks, labels, &omitted, None);
        return Fitted { blocks, context };
    }
    // Even one line does not fit: cut its text (it is then injected, so the cursor may pass it;
    // the note says how to read the rest). Otherwise this message would block the channel forever.
    let (mut blocks, omitted) = trimmed(&blocks, &order);
    let Some(bi) = blocks.iter().position(|b| !b.lines.is_empty()) else {
        return Fitted {
            context: String::new(),
            blocks,
        };
    };
    let full = blocks[bi].lines[0].text.clone();
    let seq = blocks[bi].lines[0].seq;
    let with_cut = |t: usize, blocks: &mut Vec<ChannelBlock>| {
        let mut s: String = full.chars().take(t).collect();
        s.push('\u{2026}');
        blocks[bi].lines[0].text = s;
        render(loc, blocks, labels, &omitted, Some((bi, seq)))
    };
    let fits = |t: usize| chars(&with_cut(t, &mut blocks.clone())) <= cap;
    // A cap too small for even the notes cannot be honoured; cut to nothing then.
    let t = if fits(0) {
        longest_fitting(chars(&full), fits)
    } else {
        0
    };
    let context = with_cut(t, &mut blocks);
    Fitted { blocks, context }
}

/// Largest `t` in `[0, max]` with `fits(t)`, given that size grows with `t` and `fits(0)` holds.
fn longest_fitting(max: usize, fits: impl Fn(usize) -> bool) -> usize {
    let (mut lo, mut hi) = (0, max + 1);
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if fits(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

pub async fn commit(st: &AppState, me: &Identity, p: &Prepared) -> PortResult<()> {
    for (ch, seq) in &p.advance_to {
        port::advance(st, &me.session_id, ch, *seq).await?;
    }
    Ok(())
}

/// `agent.hook {event: post-tool-use}`. Errors go to stderr and produce no output (a hook would
/// rather miss once than break the turn).
pub async fn post_tool_use(st: &AppState, me: &Identity, payload: &Value) -> HookResult {
    if let Some(t) = payload.get("tool_name").and_then(Value::as_str)
        && counts_as_progress(t)
        && let Err(e) = port::note_progress(st, &me.session_id).await
    {
        eprintln!("crewd: progress for {}: {e}", me.session_id);
    }
    let prepared = match prepare(st, me).await {
        Ok(Some(p)) => p,
        Ok(None) => return HookResult { stdout: None },
        Err(e) => {
            eprintln!("crewd: post-tool-use for {}: {e}", me.session_id);
            return HookResult { stdout: None };
        }
    };
    if let Err(e) = commit(st, me, &prepared).await {
        eprintln!("crewd: post-tool-use cursor for {}: {e}", me.session_id);
        return HookResult { stdout: None };
    }
    let stdout = prepared.context.map(|c| {
        json!({"hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext": c}})
            .to_string()
    });
    HookResult { stdout }
}

#[cfg(test)]
mod tests {
    use crewd_proto::{ChannelKind, Mention, MentionKind};
    use serde_json::Value;

    use super::*;
    use crate::agent::Role;
    use crate::agent::port;
    use crate::agent::runtime::AgentConfig;
    use crate::agent::testkit::Fixture;

    fn at(kind: MentionKind, target: Option<&str>) -> Mention {
        Mention {
            kind,
            target_id: target.map(str::to_string),
        }
    }

    async fn ctx_of(f: &Fixture, session: &str) -> Option<String> {
        let me = f.me(session).await;
        let out = post_tool_use(
            &f.state,
            &me,
            &serde_json::json!({"tool_name": "mcp__crew__read_channel"}),
        )
        .await
        .stdout?;
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        Some(
            v["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .to_string(),
        )
    }

    /// Calls the hook until nothing is left; every context must respect `cap`.
    async fn drain(f: &Fixture, session: &str, cap: usize) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(c) = ctx_of(f, session).await {
            assert!(c.chars().count() <= cap, "{} > {cap}", c.chars().count());
            out.push(c);
            assert!(out.len() < 500, "injection does not drain");
        }
        out
    }

    /// The numbers of the `<tag>-NNN` markers in `ctx`, in order of appearance.
    fn markers(ctx: &str, tag: &str) -> Vec<usize> {
        let pat = format!("{tag}-");
        ctx.match_indices(&pat)
            .filter_map(|(i, _)| ctx[i + pat.len()..].get(..3)?.parse().ok())
            .collect()
    }

    async fn lobby(f: &Fixture) -> String {
        port::lobby_of(&f.state, &f.crew).await.unwrap().id
    }

    /// The tool names whose post-tool-use call reported progress (each call run on its own).
    async fn progress_of(f: &Fixture, tools: &[&str]) -> Vec<String> {
        let me = f.me(&f.worker).await;
        let mut counted = Vec::new();
        for t in tools {
            let mut rx = port::subscribe_collab(&f.state);
            post_tool_use(&f.state, &me, &serde_json::json!({"tool_name": t})).await;
            let mut n = 0;
            while let Ok(c) = rx.try_recv() {
                if let crewd_collab::Change::Progress { session_id, kind } = c {
                    assert_eq!(
                        (session_id.as_str(), kind),
                        (f.worker.as_str(), crewd_collab::ProgressKind::ToolCall)
                    );
                    n += 1;
                }
            }
            assert!(n <= 1, "{t}: {n} progress reports");
            if n == 1 {
                counted.push(t.to_string());
            }
        }
        counted
    }

    #[tokio::test]
    async fn unread_is_injected_once() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        f.human_post(&l, "示例：今天先做 A", vec![]).await;
        let ctx = ctx_of(&f, &f.worker).await.expect("有未读");
        assert!(ctx.contains("示例：今天先做 A"), "{ctx}");
        assert!(ctx_of(&f, &f.worker).await.is_none(), "同一条至多注入一次");
    }

    #[tokio::test]
    async fn injected_lines_carry_their_seq() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        let m = f.human_post(&l, "示例：带号", vec![]).await;
        let ctx = ctx_of(&f, &f.worker).await.unwrap();
        assert!(ctx.contains(&format!("#{}", m.seq)), "{ctx}");
    }

    #[tokio::test]
    async fn only_real_work_reports_progress() {
        // C3 + B14: pure messaging / reading crew tools are not output (two sessions answering each
        // other would never be cut); everything else is, state-changing crew tools included.
        let f = Fixture::new().await;
        let passive = [
            "post",
            "read_channel",
            "search",
            "list_channels",
            "human_watching",
            "directory",
            "contact",
            "channel_listen",
            "channel_unlisten",
        ];
        let tools: Vec<String> = passive.iter().map(|t| format!("mcp__crew__{t}")).collect();
        let tools: Vec<&str> = tools.iter().map(String::as_str).collect();
        assert_eq!(progress_of(&f, &tools).await, Vec::<String>::new());
        let counted = [
            "Bash",
            "Edit",
            "mcp__computer-use__click",
            "mcp__crew__start_session",
            "mcp__crew__nudge_session",
            "mcp__crew__stop_session",
            "mcp__crew__channel_create",
            "mcp__crew__channel_invite",
            "mcp__crew__channel_archive",
            "mcp__crew__channel_transfer",
            "mcp__crew__ask",
            "mcp__crew__respond_todo",
            "mcp__crew__schedule_wakeup",
            "mcp__crew__set_session_profile",
            "mcp__crew__rename_crew",
        ];
        assert_eq!(progress_of(&f, &counted).await, counted);
    }

    #[test]
    fn crewd_tool_names_and_the_passive_list_match_the_catalog() {
        assert!(
            is_crewd_tool("mcp__crew__contact")
                && !is_crewd_tool("mcp__computer-use__click")
                && !is_crewd_tool("Edit")
        );
        for t in PASSIVE_CREW_TOOLS {
            assert!(
                crate::agent::catalog::find(t).is_some(),
                "{t} is not a tool"
            );
        }
        assert!(!counts_as_progress("mcp__crew__post"));
        assert!(counts_as_progress("mcp__crew__channel_create"));
        assert!(counts_as_progress("Bash"));
    }

    #[tokio::test]
    async fn directed_to_others_is_hidden_and_widened_is_marked() {
        let f = Fixture::new().await;
        f.add_session(&f.crew, "s-w2", Role::Worker, "另一个 worker")
            .await;
        let l = lobby(&f).await;
        f.session_post(
            &f.captain,
            &l,
            "给 w2 的活",
            vec![at(MentionKind::Session, Some("s-w2"))],
        )
        .await;
        assert!(
            ctx_of(&f, &f.worker).await.is_none(),
            "@ 了别人的不进（游标照样推过去）"
        );
        f.session_post(
            &f.captain,
            &l,
            "全组看、w2 来做",
            vec![
                at(MentionKind::Broadcast, None),
                at(MentionKind::Session, Some("s-w2")),
            ],
        )
        .await;
        let ctx = ctx_of(&f, &f.worker).await.unwrap();
        assert!(
            ctx.contains("全组看、w2 来做") && ctx.contains("另一个 worker"),
            "旁观者看得出是给谁的：{ctx}"
        );
        assert!(!ctx.contains("给 w2 的活"));
    }

    #[tokio::test]
    async fn human_mentions_are_visible_and_own_posts_are_not_echoed() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        f.session_post(
            &f.captain,
            &l,
            "@人 我做完了",
            vec![at(MentionKind::Human, None)],
        )
        .await;
        assert!(
            ctx_of(&f, &f.worker)
                .await
                .unwrap()
                .contains("@人 我做完了")
        );
        assert!(ctx_of(&f, &f.captain).await.is_none(), "自己发的不回灌");
    }

    #[tokio::test]
    async fn only_member_or_listened_channels_are_injected() {
        let f = Fixture::new().await;
        let cap = f.me(&f.captain).await;
        let topic = port::create_channel(
            &f.state,
            &cap,
            ChannelKind::Topic,
            "示例话题".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
        f.session_post(&f.captain, &topic.id, "话题里的话", vec![])
            .await;
        assert!(
            ctx_of(&f, &f.worker).await.is_none(),
            "不是成员、没收听：不注入"
        );
        port::set_listen(&f.state, &f.worker, &topic.id, None, vec![])
            .await
            .unwrap();
        ctx_of(&f, &f.worker).await; // 第一次见到这个群：M3a 先把游标钉在末尾
        f.session_post(&f.captain, &topic.id, "收听后的话", vec![])
            .await;
        assert!(ctx_of(&f, &f.worker).await.unwrap().contains("收听后的话"));
    }

    #[tokio::test]
    async fn huge_backlog_is_capped_and_the_rest_stays_unread() {
        // R6: the cap trims the NEWEST lines, so what is injected is a prefix and the cursor moves
        // only past it; the omitted ones are delivered by the next calls, never marked read.
        let f = Fixture::with_config(AgentConfig {
            inject_cap_chars: 2_000,
            ..AgentConfig::default()
        })
        .await;
        let l = lobby(&f).await;
        for i in 0..120 {
            f.human_post(
                &l,
                &format!("L-{i:03} {}", "很长的一段说明".repeat(4)),
                vec![],
            )
            .await;
        }
        let ctxs = drain(&f, &f.worker, 2_000).await;
        assert!(ctxs.len() > 1, "一次装不下，应分多次");
        assert!(markers(&ctxs[0], "L").first() == Some(&0), "最旧的先给");
        assert!(!ctxs[0].contains("L-119"), "最新的这次没带上");
        assert!(ctxs[0].contains("read_channel"), "省略要自报：{}", ctxs[0]);
        let all: Vec<usize> = ctxs.iter().flat_map(|c| markers(c, "L")).collect();
        assert_eq!(all, (0..120).collect::<Vec<_>>(), "一条不丢、不重、不乱序");
        assert!(
            !ctxs.last().unwrap().contains("read_channel"),
            "最后一批没有省略，就不该有省略说明"
        );
    }

    #[tokio::test]
    async fn trimming_across_channels_never_skips_a_message() {
        let f = Fixture::with_config(AgentConfig {
            inject_cap_chars: 1_500,
            ..AgentConfig::default()
        })
        .await;
        let cap = f.me(&f.captain).await;
        let topic = port::create_channel(
            &f.state,
            &cap,
            ChannelKind::Topic,
            "示例话题".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
        port::set_listen(&f.state, &f.worker, &topic.id, None, vec![])
            .await
            .unwrap();
        ctx_of(&f, &f.worker).await; // pin the topic cursor
        let l = lobby(&f).await;
        for i in 0..40 {
            f.session_post(
                &f.captain,
                &l,
                &format!("L-{i:03} {}", "说明".repeat(20)),
                vec![],
            )
            .await;
            f.session_post(
                &f.captain,
                &topic.id,
                &format!("T-{i:03} {}", "说明".repeat(20)),
                vec![],
            )
            .await;
        }
        let ctxs = drain(&f, &f.worker, 1_500).await;
        for tag in ["L", "T"] {
            let seen: Vec<usize> = ctxs.iter().flat_map(|c| markers(c, tag)).collect();
            assert_eq!(seen, (0..40).collect::<Vec<_>>(), "{tag}");
        }
    }

    #[tokio::test]
    async fn one_message_longer_than_the_cap_is_cut_not_stuck() {
        let f = Fixture::with_config(AgentConfig {
            inject_cap_chars: 2_000,
            ..AgentConfig::default()
        })
        .await;
        let l = lobby(&f).await;
        f.human_post(&l, &format!("L-000 {}", "很长".repeat(10_000)), vec![])
            .await;
        f.human_post(&l, "L-001 后一条", vec![]).await;
        let ctxs = drain(&f, &f.worker, 2_000).await;
        assert_eq!(ctxs.len(), 2, "{ctxs:?}");
        assert!(
            ctxs[0].contains("L-000") && ctxs[0].contains("read_channel"),
            "{}",
            ctxs[0]
        );
        assert!(!ctxs[0].contains("L-001"));
        assert!(ctxs[1].contains("L-001 后一条"), "{}", ctxs[1]);
    }

    fn block(id: &str, lines: &[(&str, i64)]) -> ChannelBlock {
        ChannelBlock {
            channel_id: id.into(),
            kind: ChannelKind::Topic,
            name: id.into(),
            crew_title: String::new(),
            lines: lines
                .iter()
                .map(|(at, seq)| crewd_collab::render::MsgLine {
                    seq: *seq,
                    at: at.to_string(),
                    sender: String::new(),
                    text: String::new(),
                    directed_to: None,
                    reply_to: None,
                })
                .collect(),
        }
    }

    #[test]
    fn removal_drops_the_globally_newest_tail_first_and_keeps_one_line() {
        let a = block(
            "a",
            &[("2026-01-01T00:00:01Z", 1), ("2026-01-01T00:00:03Z", 2)],
        );
        let b = block(
            "b",
            &[("2026-01-01T00:00:02Z", 7), ("2026-01-01T00:00:04Z", 8)],
        );
        // b's #8 (newest), a's #2, b's #7; a's #1 (the oldest) is never dropped.
        assert_eq!(removal_order(&[a, b]), [1, 0, 1]);
    }

    #[tokio::test]
    async fn dm_and_lobby_notes_use_the_channel_label() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        let lbl = crate::agent::tools::channel_label_for(
            &f.state,
            &f.worker,
            &l,
            ChannelKind::Lobby,
            "",
            "示例机组",
            crewd_collab::Locale::Zh,
        )
        .await;
        assert_eq!(
            lbl,
            crate::agent::text::r(
                crewd_collab::Locale::Zh,
                "r.lobby_label",
                &[("crew", "示例机组")]
            )
        );
        let gone = crate::agent::tools::channel_label_raw(
            crewd_collab::Locale::En,
            ChannelKind::Dm,
            "",
            "示例机组",
        );
        assert_eq!(
            gone,
            crate::agent::text::r(crewd_collab::Locale::En, "r.dm_gone", &[])
        );
    }
}
