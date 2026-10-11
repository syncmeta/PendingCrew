//! post-tool-use (spec 22.3, "hook behaviour"): unread messages after the cursor, in the channels this
//! session is in or listens to, become `additionalContext`. Visibility, the "sent to X" marks and the
//! rendering are M3a's (`cursors::unread_for`, `render::{blocks_for, injection}`); this file adds the
//! total length cap and decides how far the cursors move (the cursor is shared with wake-ups: at
//! most once per message).
//!
//! The digest (S62): per channel, other sessions' `progress` / `ack` messages that neither @ me nor
//! reply to my message are folded into one `inject.digest` line (count, senders, seq range) instead
//! of being listed; everything else (the human, the system, any other category, anything for me)
//! stays a full line. Folded messages were announced, so the cursor passes them like shown ones.
//! The cap cuts folded messages like full lines (newest first): a folded message left out is not
//! announced and stays unread, and a text with only digest lines still gets the header.
//!
//! The cap (R6): what does not fit is left out from the NEWEST end, so what is injected is a
//! contiguous prefix of each channel's unread run. Cursors move only to the highest seq actually
//! injected; omitted messages stay unread and arrive with the next call. The omission note never
//! marks anything read. Nothing is marked read that was not shown: the cursors move in ONE write
//! transaction, and not at all when the hook is already past its deadline (the helper has given up
//! and printed nothing).
use std::time::{Duration, Instant};

use crewd_collab::render::ChannelBlock;
use crewd_proto::agent::HookResult;
use serde_json::{Value, json};

use crate::AppState;
use crate::agent::Identity;
use crate::agent::catalog::{CAT_ACK, CAT_PROGRESS};
use crate::agent::port::{self, PortResult};
use crate::agent::text::{self, Locale};
use crate::agent::tools::channel_label_for;

/// The MCP server name in `mcp.json` is `crew` (see crewd-exec `prepare_claude`).
const CREWD_TOOL_PREFIX: &str = "mcp__crew__";

/// Past this since the hook call arrived, the cursors are not moved and nothing is printed: the
/// helper's own limit is 5 s (`hook_cmd::QUICK`) and claude's 15 s, after which the output is lost.
const COMMIT_DEADLINE: Duration = Duration::from_secs(3);

pub fn is_crewd_tool(tool_name: &str) -> bool {
    tool_name.starts_with(CREWD_TOOL_PREFIX)
}

/// Does a finished tool call count as output for the loop breaker (C3 + B14)? crewd's own tools
/// that only read or talk do not (`ToolSpec::passive`, derived from the catalog's annotations,
/// S76): a session answering another through `post` is not output, or two sessions replying to
/// each other would keep the breaker from ever firing. Everything else counts, an unknown
/// `mcp__crew__*` name included (fail toward counting).
pub fn counts_as_progress(tool_name: &str) -> bool {
    match tool_name.strip_prefix(CREWD_TOOL_PREFIX) {
        Some(own) => !crate::agent::catalog::find(own).is_some_and(|t| t.passive()),
        None => true,
    }
}

/// Categories folded into the digest when another session posts them (S62, Q1).
const FOLDED_CATEGORIES: &[&str] = &[CAT_PROGRESS, CAT_ACK];
/// A digest line names at most this many senders.
const DIGEST_SENDERS: usize = 5;
/// Joins sender names in a digest line (no prose: the same in both locales).
const NAME_SEP: &str = ", ";

/// Is this unread message folded into its channel's digest (S62)? Another session's progress /
/// ack that does not @ me and is not a reply to one of my messages. `replied_sender` = the sender
/// session of the message it replies to.
fn folds(me: &str, m: &crewd_proto::Message, replied_sender: Option<&str>) -> bool {
    use crewd_proto::SenderKind;
    let by_peer = matches!(m.sender_kind, SenderKind::Session | SenderKind::Captain)
        && m.sender_session_id.as_deref().is_some_and(|s| s != me);
    let at_me = m
        .mentions
        .iter()
        .any(|x| x.target_id.as_deref() == Some(me));
    by_peer
        && m.category
            .as_deref()
            .is_some_and(|c| FOLDED_CATEGORIES.contains(&c))
        && !at_me
        && replied_sender != Some(me)
}

/// One unread message of a channel's run: a full line (`folded: None`) or folded into the
/// channel's digest line (`Some(sender label)`).
#[derive(Debug, Clone)]
struct Item {
    seq: i64,
    at: String,
    folded: Option<String>,
}

/// One channel's unread run in seq order (the window `cursors::unread_for` returned). What is
/// injected of it is always a prefix: its full lines go into the channel's block, its folded
/// messages into one digest line, and the cursor moves to the end of the prefix. Full lines and
/// folded messages are cut by the cap alike (newest first), so nothing announced is skipped and
/// nothing left out is marked read.
#[derive(Debug, Clone)]
struct ChannelRun {
    channel_id: String,
    label: String,
    items: Vec<Item>,
    /// This channel's block in the rendered blocks (`None` = it has no full line).
    block: Option<usize>,
}

impl ChannelRun {
    /// The unread window came back full (`UNREAD_CAP` messages, counted before folding): more
    /// unread messages may exist beyond it, so an omitted count is only a lower bound.
    fn window_full(&self) -> bool {
        self.items.len() >= crewd_collab::cursors::UNREAD_CAP as usize
    }

    /// The digest line for the folded messages among the first `kept` items (none if there are none).
    fn digest(&self, loc: Locale, kept: usize) -> Option<String> {
        let shown: Vec<(i64, &str)> = self.items[..kept]
            .iter()
            .filter_map(|i| i.folded.as_deref().map(|n| (i.seq, n)))
            .collect();
        let (first, last) = (shown.first()?.0, shown.last()?.0);
        let mut names: Vec<&str> = Vec::new();
        for (_, n) in &shown {
            if !names.contains(n) {
                names.push(n);
            }
        }
        let mut senders = names
            .iter()
            .take(DIGEST_SENDERS)
            .copied()
            .collect::<Vec<_>>()
            .join(NAME_SEP);
        if names.len() > DIGEST_SENDERS {
            senders.push_str(NAME_SEP);
            senders.push('\u{2026}');
        }
        Some(text::r(
            loc,
            "inject.digest",
            &[
                ("channel", &self.label),
                ("count", &shown.len().to_string()),
                ("senders", &senders),
                ("first", &first.to_string()),
                ("last", &last.to_string()),
            ],
        ))
    }
}

pub struct Prepared {
    pub context: Option<String>,
    /// Per channel, the highest seq that is really in `context` (shown or announced).
    advance_to: Vec<(String, i64)>,
    snapshot: crewd_collab::cursor_reservations::Snapshot,
    causes: crewd_collab::causal::CausalSnapshot,
}

pub async fn prepare(st: &AppState, me: &Identity) -> PortResult<Option<Prepared>> {
    let loc = port::locale(st).await?;
    let cap = st.agent.config.inject_cap_chars;
    let captured = port::prepare_injection(st, &me.session_id).await?;
    let snapshot = captured.snapshot;
    let causes = captured.causes;
    let unread = captured.blocks;
    if unread.iter().all(|(_, v)| v.is_empty()) {
        return Ok(None);
    }
    // Who wrote the messages these reply to (a reply to me is never folded).
    let replied: Vec<String> = unread
        .iter()
        .flat_map(|(_, v)| v.iter().filter_map(|m| m.in_reply_to.clone()))
        .collect();
    let replied = port::message_senders(st, replied).await?;
    let mut runs = Vec::new();
    let mut full_unread = Vec::new();
    for (channel_id, msgs) in unread {
        if msgs.is_empty() {
            continue;
        }
        let mut items = Vec::with_capacity(msgs.len());
        let mut full = Vec::new();
        for m in msgs {
            let to = m
                .in_reply_to
                .as_ref()
                .and_then(|r| replied.get(r).cloned().flatten());
            let folded = if folds(&me.session_id, &m, to.as_deref()) {
                Some(port::sender_label(st, &m, loc).await?)
            } else {
                None
            };
            items.push(Item {
                seq: m.seq,
                at: m.created_at.clone(),
                folded: folded.clone(),
            });
            if folded.is_none() {
                full.push(m);
            }
        }
        let label = match port::channel(st, &channel_id).await? {
            Some(c) => {
                channel_label_for(
                    st,
                    &me.session_id,
                    &c.id,
                    c.kind,
                    &c.name,
                    &c.crew_title,
                    loc,
                )
                .await
            }
            None => String::new(),
        };
        runs.push(ChannelRun {
            channel_id: channel_id.clone(),
            label,
            items,
            block: None,
        });
        if !full.is_empty() {
            full_unread.push((channel_id, full));
        }
    }
    let blocks = port::blocks(st, &me.session_id, loc, full_unread).await?;
    for r in &mut runs {
        r.block = blocks.iter().position(|b| b.channel_id == r.channel_id);
    }
    let fitted = fit(loc, blocks, &runs, cap);
    // Nothing shown: nothing announced, no cursor moves.
    if fitted.context.trim().is_empty() {
        return Ok(Some(Prepared {
            context: None,
            advance_to: vec![],
            causes: causes.select(&[])?,
            snapshot,
        }));
    }
    let advance_to: Vec<(String, i64)> = runs
        .iter()
        .zip(&fitted.kept)
        .filter(|(_, k)| **k > 0)
        .map(|(r, k)| (r.channel_id.clone(), r.items[*k - 1].seq))
        .collect();
    Ok(Some(Prepared {
        context: Some(fitted.context),
        causes: causes.select(&advance_to)?,
        advance_to,
        snapshot,
    }))
}

struct Fitted {
    /// Per run, how many of its items are shown or announced (a prefix).
    kept: Vec<usize>,
    context: String,
}

/// The run items in the order they are dropped: always the newest tail among all channels (`at`,
/// then seq), full or folded alike, so every channel keeps a prefix. The last remaining item is
/// never dropped (every injection makes progress).
fn removal_order(runs: &[ChannelRun]) -> Vec<usize> {
    let mut tails: Vec<usize> = runs.iter().map(|r| r.items.len()).collect();
    let total: usize = tails.iter().sum();
    let mut order = Vec::new();
    while order.len() + 1 < total {
        let key = |i: usize| {
            let it = &runs[i].items[tails[i] - 1];
            (it.at.clone(), it.seq)
        };
        let Some(i) = (0..runs.len())
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

/// The blocks and per-run prefixes after dropping the items in `order`.
fn trimmed(
    blocks: &[ChannelBlock],
    runs: &[ChannelRun],
    order: &[usize],
) -> (Vec<ChannelBlock>, Vec<usize>) {
    let mut out: Vec<ChannelBlock> = blocks.to_vec();
    let mut kept: Vec<usize> = runs.iter().map(|r| r.items.len()).collect();
    for &r in order {
        kept[r] -= 1;
        if runs[r].items[kept[r]].folded.is_none()
            && let Some(b) = runs[r].block
        {
            out[b].lines.pop();
        }
    }
    (out, kept)
}

/// The injected text: the blocks, then one digest line per channel with folded messages in its
/// prefix, then the cut / omission notes. A text without any full line still gets the header.
/// `cut` = (run, seq) of a message shown only in part.
fn render(
    loc: Locale,
    blocks: &[ChannelBlock],
    runs: &[ChannelRun],
    kept: &[usize],
    cut: Option<(usize, i64)>,
) -> String {
    let body = port::injection(loc, blocks);
    let mut notes: Vec<String> = runs
        .iter()
        .zip(kept)
        .filter_map(|(r, k)| r.digest(loc, *k))
        .collect();
    if let Some((ri, seq)) = cut {
        notes.push(text::r(
            loc,
            "inject.truncated",
            &[
                ("channel", &runs[ri].label),
                ("id", &runs[ri].channel_id),
                ("seq", &seq.to_string()),
                ("next", &(seq + 1).to_string()),
            ],
        ));
    }
    for (r, k) in runs.iter().zip(kept) {
        let n = r.items.len() - k;
        if n == 0 {
            continue;
        }
        let key = if r.window_full() {
            "inject.omitted_at_least"
        } else {
            "inject.omitted"
        };
        notes.push(text::r(
            loc,
            key,
            &[("channel", &r.label), ("count", &n.to_string())],
        ));
    }
    if notes.is_empty() {
        return body;
    }
    if body.is_empty() {
        return format!("{}\n\n{}", port::injection_header(loc), notes.join("\n"));
    }
    format!("{body}\n\n{}", notes.join("\n"))
}

/// Claude Code's 10,000 limit is a JS string length: UTF-16 code units, not chars or bytes.
fn units(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Smallest `k` in `(lo, hi]` for which `fits(k)`, given that `fits(hi)`, `!fits(lo)` and that
/// `fits` is monotone (once true, true for every larger `k`).
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

fn fit(loc: Locale, blocks: Vec<ChannelBlock>, runs: &[ChannelRun], cap: usize) -> Fitted {
    let order = removal_order(runs);
    let n = order.len();
    // The text with all its notes is NOT monotone in `k` (the first drop in a channel adds a
    // note), so a plain binary search could skip a fitting `k`. The body alone is monotone and
    // the notes only add to it: no `k` below the body's own fit can fit; from there on, take the
    // first `k` that fits with its notes (a few steps: the notes are short).
    let body = |k: usize| {
        let (b, _) = trimmed(&blocks, runs, &order[..k]);
        units(&port::injection(loc, &b))
    };
    let size = |k: usize| {
        let (b, kept) = trimmed(&blocks, runs, &order[..k]);
        units(&render(loc, &b, runs, &kept, None))
    };
    let k_body = if body(n) > cap {
        None
    } else if body(0) <= cap {
        Some(0)
    } else {
        Some(first_fit(0, n, |k| body(k) <= cap))
    };
    if let Some(k) = k_body.and_then(|k0| (k0..=n).find(|k| size(*k) <= cap)) {
        let (blocks, kept) = trimmed(&blocks, runs, &order[..k]);
        let context = render(loc, &blocks, runs, &kept, None);
        return Fitted { kept, context };
    }
    // No `k` fits: even one item does not. If it is a full line, cut its text (it is then
    // injected, so the cursor may pass it; the note says how to read it in full); otherwise this
    // message would block its channel forever. A lone digest line is shown as it is (a cap too
    // small for one digest line cannot be honoured).
    let (mut blocks, kept) = trimmed(&blocks, runs, &order);
    let Some(ri) = (0..runs.len()).find(|r| kept[*r] > 0) else {
        return Fitted {
            context: String::new(),
            kept,
        };
    };
    let (Some(bi), None) = (runs[ri].block, &runs[ri].items[kept[ri] - 1].folded) else {
        let context = render(loc, &blocks, runs, &kept, None);
        return Fitted { kept, context };
    };
    let whole = blocks[bi].lines[0].text.clone();
    let seq = blocks[bi].lines[0].seq;
    let with_cut = |t: usize, blocks: &mut Vec<ChannelBlock>| {
        let mut s: String = whole.chars().take(t).collect();
        s.push('\u{2026}');
        blocks[bi].lines[0].text = s;
        render(loc, blocks, runs, &kept, Some((ri, seq)))
    };
    let fits = |t: usize| units(&with_cut(t, &mut blocks.clone())) <= cap;
    // A cap too small for even the notes cannot be honoured; cut to nothing then.
    let t = if fits(0) {
        longest_fitting(whole.chars().count(), fits)
    } else {
        0
    };
    let context = with_cut(t, &mut blocks);
    Fitted { kept, context }
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

/// All cursors of one injection in one transaction.
pub async fn commit(
    st: &AppState,
    _me: &Identity,
    p: &Prepared,
) -> PortResult<crewd_collab::cursor_reservations::Decision<()>> {
    port::commit_injection(st, &p.snapshot, &p.causes, p.advance_to.clone()).await
}

/// `agent.hook {event: post-tool-use}`; also serves `PostToolUseFailure` (same helper command,
/// same handler; the event name in the payload picks the `hookEventName` of the output). Errors go
/// to stderr and produce no output (a hook would rather miss once than break the turn).
pub async fn post_tool_use(st: &AppState, me: &Identity, payload: &Value) -> HookResult {
    post_tool_use_since(st, me, payload, Instant::now()).await
}

async fn post_tool_use_since(
    st: &AppState,
    me: &Identity,
    payload: &Value,
    started: Instant,
) -> HookResult {
    if let Some(t) = payload.get("tool_name").and_then(Value::as_str)
        && counts_as_progress(t)
        && let Err(e) = port::note_progress(st, &me.session_id).await
    {
        eprintln!("crewd: progress for {}: {e}", me.session_id);
    }
    // Parallel tool calls fire parallel hooks: one at a time per session, or both would read the
    // same unread messages.
    let lock = st.agent.session_lock(&me.session_id);
    let _held = lock.lock().await;
    #[cfg(feature = "testkit")]
    let observed_call = crate::agent::testkit::begin_injection_observation(st);
    for attempt in 0..2 {
        let prepared = match prepare(st, me).await {
            Ok(Some(p)) => p,
            Ok(None) => {
                #[cfg(feature = "testkit")]
                crate::agent::testkit::record_injection_completion(
                    st,
                    &me.session_id,
                    payload,
                    crate::agent::testkit::InjectionOutcome::Empty,
                );
                return HookResult { stdout: None };
            }
            Err(e) => {
                #[cfg(feature = "testkit")]
                crate::agent::testkit::record_injection_completion(
                    st,
                    &me.session_id,
                    payload,
                    crate::agent::testkit::InjectionOutcome::InternalError,
                );
                eprintln!("crewd: post-tool-use for {}: {e}", me.session_id);
                return HookResult { stdout: None };
            }
        };
        #[cfg(feature = "testkit")]
        crate::agent::testkit::pause_caller(
            st,
            crate::agent::testkit::CallerStage::HookPrepared,
            &me.session_id,
            prepared.advance_to.clone(),
            prepared.context.clone().unwrap_or_default(),
        )
        .await;
        if started.elapsed() > COMMIT_DEADLINE {
            #[cfg(feature = "testkit")]
            crate::agent::testkit::record_injection_completion(
                st,
                &me.session_id,
                payload,
                crate::agent::testkit::InjectionOutcome::Deadline,
            );
            eprintln!(
                "crewd: post-tool-use for {} took too long; nothing injected, cursors kept",
                me.session_id
            );
            return HookResult { stdout: None };
        }
        let committed = commit(st, me, &prepared).await;
        #[cfg(feature = "testkit")]
        if let Some(call) = observed_call {
            use crate::agent::testkit::{InjectionAttempt, InjectionCommitOutcome};
            let outcome = match &committed {
                Ok(crewd_collab::cursor_reservations::Decision::Applied(())) => {
                    InjectionCommitOutcome::Applied
                }
                Ok(crewd_collab::cursor_reservations::Decision::Stale) => {
                    InjectionCommitOutcome::Stale
                }
                Ok(crewd_collab::cursor_reservations::Decision::Busy) => {
                    InjectionCommitOutcome::Busy
                }
                Err(_) => InjectionCommitOutcome::InternalError,
            };
            let event = match payload["hook_event_name"].as_str() {
                Some("PostToolUseFailure") => "PostToolUseFailure",
                _ => "PostToolUse",
            };
            crate::agent::testkit::record_injection_attempt(
                st,
                observed_call,
                InjectionAttempt {
                    call,
                    session: me.session_id.clone(),
                    event: event.into(),
                    attempt,
                    targets: prepared.advance_to.clone(),
                    outcome,
                },
            );
        }
        match committed {
            Ok(crewd_collab::cursor_reservations::Decision::Applied(())) => {
                let event = match payload.get("hook_event_name").and_then(Value::as_str) {
                    Some("PostToolUseFailure") => "PostToolUseFailure",
                    _ => "PostToolUse",
                };
                let stdout = prepared.context.map(|c| {
                    json!({"hookSpecificOutput": {
                        "hookEventName": event, "additionalContext": c,
                    }})
                    .to_string()
                });
                #[cfg(feature = "testkit")]
                crate::agent::testkit::record_injection_completion(
                    st,
                    &me.session_id,
                    payload,
                    if stdout.is_some() {
                        crate::agent::testkit::InjectionOutcome::Committed
                    } else {
                        crate::agent::testkit::InjectionOutcome::Empty
                    },
                );
                return HookResult { stdout };
            }
            Ok(decision) => {
                // A competing wake/hook changed ownership. Reprepare once within the original
                // budget; never print the old context or turn a storage error into a normal refusal.
                if attempt == 0 && started.elapsed() < COMMIT_DEADLINE {
                    continue;
                }
                #[cfg(feature = "testkit")]
                crate::agent::testkit::record_injection_completion(
                    st,
                    &me.session_id,
                    payload,
                    match decision {
                        crewd_collab::cursor_reservations::Decision::Stale => {
                            crate::agent::testkit::InjectionOutcome::Stale
                        }
                        crewd_collab::cursor_reservations::Decision::Busy => {
                            crate::agent::testkit::InjectionOutcome::Busy
                        }
                        crewd_collab::cursor_reservations::Decision::Applied(()) => unreachable!(),
                    },
                );
                #[cfg(not(feature = "testkit"))]
                let _ = decision;
                return HookResult { stdout: None };
            }
            Err(e) => {
                #[cfg(feature = "testkit")]
                crate::agent::testkit::record_injection_completion(
                    st,
                    &me.session_id,
                    payload,
                    crate::agent::testkit::InjectionOutcome::InternalError,
                );
                eprintln!("crewd: post-tool-use cursor for {}: {e}", me.session_id);
                return HookResult { stdout: None };
            }
        }
    }
    unreachable!("two bounded attempts always return")
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
            let units = c.encode_utf16().count();
            assert!(units <= cap, "{units} > {cap} UTF-16 units");
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
            "directory",
            "channel_listen",
            "get_quota",
            "inspect_session",
            "list_sessions",
        ];
        let tools: Vec<String> = passive.iter().map(|t| format!("mcp__crew__{t}")).collect();
        let tools: Vec<&str> = tools.iter().map(String::as_str).collect();
        assert_eq!(progress_of(&f, &tools).await, Vec::<String>::new());
        let counted = [
            "Bash",
            "Edit",
            "mcp__computer-use__click",
            "mcp__crew__no_such_tool",
            "mcp__crew__delegation_policy",
            "mcp__crew__crew_structure",
            "mcp__crew__change_workdir",
            "mcp__crew__handoff_captain",
            "mcp__crew__start_session",
            "mcp__crew__nudge_session",
            "mcp__crew__stop_session",
            "mcp__crew__channel_create",
            "mcp__crew__channel_invite",
            "mcp__crew__channel_archive",
            "mcp__crew__channel_owner",
            "mcp__crew__channel_leave",
            "mcp__crew__ask",
            "mcp__crew__respond_todo",
            "mcp__crew__schedule_wakeup",
            "mcp__crew__set_session_profile",
            "mcp__crew__rename_crew",
        ];
        assert_eq!(progress_of(&f, &counted).await, counted);
    }

    #[test]
    fn crewd_tool_names_and_passive_comes_from_the_catalog() {
        assert!(
            is_crewd_tool("mcp__crew__directory")
                && !is_crewd_tool("mcp__computer-use__click")
                && !is_crewd_tool("Edit")
        );
        for t in crate::agent::catalog::CATALOG {
            assert_eq!(
                counts_as_progress(&format!("mcp__crew__{}", t.name)),
                !t.passive(),
                "{}",
                t.name
            );
        }
        assert!(!counts_as_progress("mcp__crew__post"));
        assert!(counts_as_progress("mcp__crew__channel_create"));
        assert!(
            counts_as_progress("mcp__crew__read_delegation_policy"),
            "a removed name counts"
        );
        assert!(counts_as_progress("Bash"));
    }

    /// §24.2: @ only decides who is woken. Everything others post in my channels is injected, and
    /// each line says whom it mentions (the reader itself reads as "you").
    #[tokio::test]
    async fn everything_others_post_is_injected_and_mentions_are_marked() {
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
        let ctx = ctx_of(&f, &f.worker).await.unwrap();
        assert!(
            ctx.contains("给 w2 的活") && ctx.contains("（@") && ctx.contains("另一个 worker"),
            "@ 了别人的照样进，标出 @ 了谁：{ctx}"
        );
        f.session_post(
            &f.captain,
            &l,
            "你来做",
            vec![at(MentionKind::Session, Some(f.worker.as_str()))],
        )
        .await;
        let ctx = ctx_of(&f, &f.worker).await.unwrap();
        assert!(ctx.contains("你来做") && ctx.contains("（@你）"), "{ctx}");
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
            ..Default::default()
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
        let (pre, _) = text::get(crewd_collab::Locale::Zh, "inject.omitted_at_least")
            .split_once("{count}")
            .unwrap();
        assert!(
            !ctxs[0].contains(pre.split_once("{channel}").unwrap().1),
            "窗口没满，不说「至少」"
        );
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
            ..Default::default()
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
            ..Default::default()
        })
        .await;
        let l = lobby(&f).await;
        let m0 = f
            .human_post(&l, &format!("L-000 {}", "很长".repeat(10_000)), vec![])
            .await;
        f.human_post(&l, "L-001 后一条", vec![]).await;
        let ctxs = drain(&f, &f.worker, 2_000).await;
        assert_eq!(ctxs.len(), 2, "{ctxs:?}");
        assert!(ctxs[0].contains("L-000"), "{}", ctxs[0]);
        // The note names the channel and the exact call that returns this message in full.
        let hint = format!(
            "read_channel(channel=\"{l}\", before={}, limit=1)",
            m0.seq + 1
        );
        assert!(ctxs[0].contains(&hint), "{hint}\n{}", ctxs[0]);
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
                    mentions: vec![],
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
        assert_eq!(removal_order(&runs_of(&[a, b])), [1, 0, 1]);
    }

    #[tokio::test]
    async fn a_late_commit_is_skipped_and_nothing_is_marked_read() {
        // Nothing may be marked read without having been shown: past the deadline the hook prints
        // nothing, so the cursor must stay.
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        f.human_post(&l, "示例：迟到的一条", vec![]).await;
        let me = f.me(&f.worker).await;
        let late = Instant::now()
            .checked_sub(COMMIT_DEADLINE + Duration::from_secs(1))
            .expect("monotonic clock far enough from its origin");
        let r = post_tool_use_since(&f.state, &me, &json!({"tool_name": "Bash"}), late).await;
        assert!(r.stdout.is_none());
        let ctx = ctx_of(&f, &f.worker).await.expect("still unread");
        assert!(ctx.contains("示例：迟到的一条"), "{ctx}");
    }

    #[tokio::test]
    async fn cursor_advances_are_one_transaction() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        let m = f.human_post(&l, "示例：原子", vec![]).await;
        let me = f.me(&f.worker).await;
        let captured = port::prepare_injection(&f.state, &me.session_id)
            .await
            .unwrap();
        let p = Prepared {
            context: None,
            snapshot: captured.snapshot,
            causes: captured.causes,
            advance_to: vec![(l.clone(), m.seq), ("no-such-channel".into(), 1)],
        };
        let err = commit(&f.state, &me, &p).await;
        assert!(err.is_err(), "a bad target fails the commit");
        let ctx = ctx_of(&f, &f.worker)
            .await
            .expect("the good cursor did not move either");
        assert!(ctx.contains("示例：原子"), "{ctx}");
    }

    #[tokio::test]
    async fn parallel_hooks_do_not_inject_the_same_messages_twice() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        for i in 0..5 {
            f.human_post(&l, &format!("L-{i:03}"), vec![]).await;
        }
        let (a, b, c) = tokio::join!(
            ctx_of(&f, &f.worker),
            ctx_of(&f, &f.worker),
            ctx_of(&f, &f.worker)
        );
        let got: Vec<String> = [a, b, c].into_iter().flatten().collect();
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(markers(&got[0], "L"), [0, 1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn the_cap_counts_utf16_units() {
        // Claude Code's limit is a JS string length: an astral character is two units.
        let f = Fixture::with_config(AgentConfig {
            inject_cap_chars: 2_000,
            ..Default::default()
        })
        .await;
        let l = lobby(&f).await;
        f.human_post(&l, &format!("L-000 {}", "😀".repeat(5_000)), vec![])
            .await;
        f.human_post(&l, &format!("L-001 {}", "😀".repeat(300)), vec![])
            .await;
        let ctxs = drain(&f, &f.worker, 2_000).await; // asserts UTF-16 length per batch
        assert!(ctxs.len() >= 2, "{}", ctxs.len());
        assert!(ctxs.iter().any(|c| c.contains("L-001")));
    }

    #[tokio::test]
    async fn a_full_window_says_at_least() {
        let f = Fixture::with_config(AgentConfig {
            inject_cap_chars: 3_000,
            ..Default::default()
        })
        .await;
        let l = lobby(&f).await;
        for i in 0..(crewd_collab::cursors::UNREAD_CAP as usize + 5) {
            f.human_post(&l, &format!("L-{:03}", i % 1000), vec![])
                .await;
        }
        let first = ctx_of(&f, &f.worker).await.unwrap();
        let (_, tail) = text::get(crewd_collab::Locale::Zh, "inject.omitted_at_least")
            .split_once("{count}")
            .unwrap();
        assert!(first.contains(tail.lines().next().unwrap()), "{first}");
        assert!(first.contains("至少"), "{first}");
    }

    #[test]
    fn notes_do_not_stack_quotes_on_the_label() {
        for loc in [crewd_collab::Locale::Zh, crewd_collab::Locale::En] {
            let label =
                crate::agent::tools::channel_label_raw(loc, ChannelKind::Lobby, "", "示例机组");
            for key in [
                "inject.omitted",
                "inject.omitted_at_least",
                "inject.truncated",
            ] {
                let note = text::r(
                    loc,
                    key,
                    &[
                        ("channel", &label),
                        ("count", "3"),
                        ("seq", "7"),
                        ("next", "8"),
                        ("id", "c1"),
                    ],
                );
                assert!(
                    !note.contains("「「") && !note.contains("\"\"") && !note.contains("\"lobby"),
                    "{loc:?} {key}: {note}"
                );
            }
        }
    }

    /// Sizes are not monotone in the number of dropped lines: the first drop in a channel adds a
    /// note. Binary search over `k` would then skip a fitting `k`.
    #[test]
    fn probe_nonmonotone_fit() {
        let big = "很长的说明".repeat(40);
        let mut a = block(
            "a",
            &[("2026-01-01T00:00:01Z", 1), ("2026-01-01T00:00:08Z", 2)],
        );
        let mut b = block(
            "b",
            &[
                ("2026-01-01T00:00:06Z", 7),
                ("2026-01-01T00:00:07Z", 8),
                ("2026-01-01T00:00:09Z", 9),
            ],
        );
        for l in &mut b.lines {
            l.text = big.clone();
        }
        a.name = "示例话题甲甲甲甲甲甲甲甲甲甲甲甲甲甲甲甲甲甲".into();
        b.name = "示例话题乙乙乙乙乙乙乙乙乙乙乙乙乙乙乙乙乙乙".into();
        let blocks = vec![a, b];
        let runs = runs_of(&blocks);
        let loc = crewd_collab::Locale::Zh;
        let order = removal_order(&runs);
        assert_eq!(order, [1, 0, 1, 1]);
        let size = |k: usize| {
            let (b, kept) = trimmed(&blocks, &runs, &order[..k]);
            units(&render(loc, &b, &runs, &kept, None))
        };
        let cap = size(1);
        assert!(
            size(0) > cap && size(2) > cap,
            "{} {} {}",
            size(0),
            cap,
            size(2)
        );
        assert!(
            size(3) <= cap || size(4) <= cap,
            "binary search would settle here"
        );
        let fitted = fit(loc, blocks.clone(), &runs, cap);
        let kept: usize = fitted.kept.iter().sum();
        assert_eq!(kept, 4, "the smallest fitting k keeps the most lines");
    }

    /// Runs for blocks of full lines only (label = the block name).
    fn runs_of(blocks: &[ChannelBlock]) -> Vec<ChannelRun> {
        blocks
            .iter()
            .enumerate()
            .map(|(i, b)| ChannelRun {
                channel_id: b.channel_id.clone(),
                label: a_label(b),
                items: b
                    .lines
                    .iter()
                    .map(|l| Item {
                        seq: l.seq,
                        at: l.at.clone(),
                        folded: None,
                    })
                    .collect(),
                block: Some(i),
            })
            .collect()
    }

    fn a_label(b: &ChannelBlock) -> String {
        b.name.clone()
    }

    #[tokio::test]
    async fn a_failure_event_injects_and_counts_like_post_tool_use() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        f.human_post(&l, "示例：失败时也带上", vec![]).await;
        let me = f.me(&f.worker).await;
        let mut rx = port::subscribe_collab(&f.state);
        let out = post_tool_use(
            &f.state,
            &me,
            &json!({"tool_name": "Bash", "hook_event_name": "PostToolUseFailure", "error": "x"}),
        )
        .await
        .stdout
        .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["hookSpecificOutput"]["hookEventName"],
            "PostToolUseFailure"
        );
        assert!(
            v["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .contains("示例：失败时也带上")
        );
        assert!(
            std::iter::from_fn(|| rx.try_recv().ok())
                .any(|c| matches!(c, crewd_collab::Change::Progress { .. }))
        );
    }

    #[tokio::test]
    async fn a_live_dm_is_called_by_its_peer_and_only_a_gone_peer_is_dissolved() {
        let f = Fixture::new().await;
        let w = f.me(&f.worker).await;
        let (dm, _) = port::ensure_dm(&f.state, &w, &f.captain).await.unwrap();
        let loc = crewd_collab::Locale::Zh;
        let label = crate::agent::tools::channel_label_for(
            &f.state,
            &f.worker,
            &dm.id,
            ChannelKind::Dm,
            "",
            "示例机组",
            loc,
        )
        .await;
        assert!(label.contains("示例机长"), "{label}");
        let gone = text::r(loc, "r.dm_gone", &[]);
        let generic = text::r(loc, "r.dm_generic", &[]);
        assert_ne!(gone, generic);
        assert_eq!(
            crate::agent::tools::channel_label_raw(loc, ChannelKind::Dm, "", "x"),
            generic,
            "no viewer, no DB: neutral label, not 'dissolved'"
        );
        // A DB error is not a gone peer: unknown channel id -> display_label errors -> generic.
        let err_label = crate::agent::tools::channel_label_for(
            &f.state,
            &f.worker,
            "no-such-channel",
            ChannelKind::Dm,
            "",
            "x",
            loc,
        )
        .await;
        assert_eq!(err_label, generic);
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
            crate::agent::text::r(crewd_collab::Locale::En, "r.dm_generic", &[])
        );
    }

    // --- S62 injection digest: other sessions' progress / ack fold into one line per channel ---

    async fn peer_post(
        f: &Fixture,
        who: &str,
        ch: &str,
        text: &str,
        category: Option<&str>,
        mentions: Vec<Mention>,
        reply_to: Option<String>,
    ) -> crewd_proto::Message {
        port::post(
            &f.state,
            port::PostReq {
                channel_id: ch.into(),
                sender: port::PostSender::Session(who.into()),
                text: text.into(),
                mentions,
                in_reply_to: reply_to,
                category: category.map(str::to_string),
                attachments: vec![],
            },
        )
        .await
        .unwrap()
        .message
    }

    /// The `inject.digest` lines in `ctx` (fixed parts of the template all present).
    fn digest_lines(ctx: &str) -> Vec<&str> {
        let probe = text::r(
            crewd_collab::Locale::Zh,
            "inject.digest",
            &[
                ("channel", "\u{1}"),
                ("count", "\u{1}"),
                ("senders", "\u{1}"),
                ("first", "\u{1}"),
                ("last", "\u{1}"),
            ],
        );
        let parts: Vec<&str> = probe.split('\u{1}').filter(|p| !p.is_empty()).collect();
        ctx.lines()
            .filter(|l| parts.iter().all(|p| l.contains(p)))
            .collect()
    }

    fn digest_line(label: &str, count: usize, senders: &str, first: i64, last: i64) -> String {
        text::r(
            crewd_collab::Locale::Zh,
            "inject.digest",
            &[
                ("channel", label),
                ("count", &count.to_string()),
                ("senders", senders),
                ("first", &first.to_string()),
                ("last", &last.to_string()),
            ],
        )
    }

    fn lobby_label() -> String {
        crate::agent::tools::channel_label_raw(
            crewd_collab::Locale::Zh,
            ChannelKind::Lobby,
            "",
            "示例机组",
        )
    }

    async fn label_of(f: &Fixture, session: &str) -> String {
        let m = f
            .session_post(session, &lobby(f).await, "示例：标签探针", vec![])
            .await;
        port::sender_label(&f.state, &m, crewd_collab::Locale::Zh)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn peer_progress_and_ack_fold_into_one_digest_line() {
        let f = Fixture::new().await;
        f.add_session(&f.crew, "s-w2", Role::Worker, "另一个 worker")
            .await;
        let w2 = label_of(&f, "s-w2").await;
        let l = lobby(&f).await;
        ctx_of(&f, &f.worker).await; // the probe above
        let a = peer_post(&f, "s-w2", &l, "P-001 进度", Some("progress"), vec![], None).await;
        peer_post(&f, "s-w2", &l, "A-001 收到", Some("ack"), vec![], None).await;
        let c = peer_post(
            &f,
            &f.captain,
            &l,
            "P-002 进度",
            Some("progress"),
            vec![],
            None,
        )
        .await;
        let cap = label_of(&f, &f.captain).await;
        let ctx = ctx_of(&f, &f.worker).await.expect("the digest is injected");
        for t in ["P-001", "A-001", "P-002"] {
            assert!(!ctx.contains(t), "{t} is folded, not listed: {ctx}");
        }
        assert_eq!(digest_lines(&ctx).len(), 1, "one line per channel: {ctx}");
        let want = digest_line(&lobby_label(), 3, &format!("{w2}, {cap}"), a.seq, c.seq);
        assert!(ctx.contains(&want), "{want}\n{ctx}");
        assert!(ctx.contains("read_channel"), "{ctx}");
        // The captain's own probe after them is a full line: only it is listed.
        assert!(ctx.contains("示例：标签探针"), "{ctx}");
    }

    #[tokio::test]
    async fn human_done_question_and_mentions_of_me_are_never_folded() {
        let f = Fixture::new().await;
        f.add_session(&f.crew, "s-w2", Role::Worker, "另一个 worker")
            .await;
        let l = lobby(&f).await;
        f.human_post(&l, "H-001 人说的", vec![]).await;
        for (t, cat) in [
            ("D-001", "done"),
            ("Q-001", "question"),
            ("B-001", "blocked"),
            ("F-001", "finding"),
            ("O-001", "handoff"),
            ("N-001", "note"),
        ] {
            peer_post(&f, "s-w2", &l, t, Some(cat), vec![], None).await;
        }
        peer_post(&f, "s-w2", &l, "U-001 没分类", None, vec![], None).await;
        peer_post(
            &f,
            "s-w2",
            &l,
            "M-001 @我的进度",
            Some("progress"),
            vec![at(MentionKind::Session, Some(f.worker.as_str()))],
            None,
        )
        .await;
        peer_post(
            &f,
            "s-w2",
            &l,
            "K-001 @机长的收到",
            Some("ack"),
            vec![at(MentionKind::Captain, None)],
            None,
        )
        .await;
        port::post(
            &f.state,
            port::PostReq {
                channel_id: l.clone(),
                sender: port::PostSender::System,
                text: "S-001 系统".into(),
                mentions: vec![],
                in_reply_to: None,
                category: None,
                attachments: vec![],
            },
        )
        .await
        .unwrap();
        let ctx = ctx_of(&f, &f.worker).await.unwrap();
        for t in [
            "H-001", "D-001", "Q-001", "B-001", "F-001", "O-001", "N-001", "U-001", "M-001",
            "S-001",
        ] {
            assert!(ctx.contains(t), "{t} stays a full line: {ctx}");
        }
        // @captain is not @ the worker: folded for it.
        assert!(!ctx.contains("K-001"), "{ctx}");
        assert_eq!(digest_lines(&ctx).len(), 1, "{ctx}");
        // ... but for the captain it is an @ of itself: full; the worker's @ is folded there.
        let cap = ctx_of(&f, &f.captain).await.unwrap();
        assert!(cap.contains("K-001") && !cap.contains("M-001"), "{cap}");
    }

    #[tokio::test]
    async fn reply_to_my_message_is_never_folded() {
        let f = Fixture::new().await;
        f.add_session(&f.crew, "s-w2", Role::Worker, "另一个 worker")
            .await;
        let cap = f.me(&f.captain).await;
        let t = port::create_channel(
            &f.state,
            &cap,
            ChannelKind::Topic,
            "示例话题".into(),
            None,
            vec![
                port::MemberRef::Session(f.worker.clone()),
                port::MemberRef::Session("s-w2".into()),
            ],
        )
        .await
        .unwrap();
        let mine = f.session_post(&f.worker, &t.id, "我的问题", vec![]).await;
        // I leave and listen instead: a reply then carries no automatic @ of me, so only the
        // reply rule can keep it full.
        let w = f.me(&f.worker).await;
        port::remove_member(
            &f.state,
            &w,
            &t.id,
            port::MemberRef::Session(f.worker.clone()),
        )
        .await
        .unwrap();
        port::set_listen(&f.state, &f.worker, &t.id, None, vec![])
            .await
            .unwrap();
        ctx_of(&f, &f.worker).await;
        let reply = peer_post(
            &f,
            "s-w2",
            &t.id,
            "R-001 回你",
            Some("ack"),
            vec![],
            Some(mine.id.clone()),
        )
        .await;
        assert!(
            !reply
                .mentions
                .iter()
                .any(|m| m.target_id.as_deref() == Some(f.worker.as_str())),
            "the reply must not @ me for this test: {:?}",
            reply.mentions
        );
        peer_post(
            &f,
            "s-w2",
            &t.id,
            "R-002 别的收到",
            Some("ack"),
            vec![],
            None,
        )
        .await;
        let ctx = ctx_of(&f, &f.worker).await.unwrap();
        assert!(
            ctx.contains("R-001"),
            "a reply to my message stays full: {ctx}"
        );
        assert!(!ctx.contains("R-002"), "{ctx}");
        assert_eq!(digest_lines(&ctx).len(), 1, "{ctx}");
    }

    #[tokio::test]
    async fn cursor_moves_past_folded_messages() {
        let f = Fixture::new().await;
        f.add_session(&f.crew, "s-w2", Role::Worker, "另一个 worker")
            .await;
        let l = lobby(&f).await;
        peer_post(&f, "s-w2", &l, "P-001", Some("progress"), vec![], None).await;
        f.human_post(&l, "H-001 中间一条", vec![]).await;
        let last = peer_post(&f, "s-w2", &l, "P-002", Some("progress"), vec![], None).await;
        let ctx = ctx_of(&f, &f.worker).await.unwrap();
        assert!(
            ctx.contains("H-001") && digest_lines(&ctx).len() == 1,
            "{ctx}"
        );
        assert!(ctx_of(&f, &f.worker).await.is_none(), "announced = read");
        assert_eq!(
            port::cursor(&f.state, &f.worker, &l).await.unwrap(),
            Some(last.seq)
        );
    }

    #[tokio::test]
    async fn a_channel_with_only_folded_messages_still_shows_its_digest() {
        let f = Fixture::new().await;
        f.add_session(&f.crew, "s-w2", Role::Worker, "另一个 worker")
            .await;
        let l = lobby(&f).await;
        let p = peer_post(&f, "s-w2", &l, "P-001", Some("progress"), vec![], None).await;
        let ctx = ctx_of(&f, &f.worker)
            .await
            .expect("a digest alone is injected");
        assert!(!ctx.contains("P-001"), "{ctx}");
        assert!(
            ctx.starts_with(&port::injection_header(crewd_collab::Locale::Zh)),
            "a digest-only injection still has the header: {ctx}"
        );
        let w2 = port::sender_label(&f.state, &p, crewd_collab::Locale::Zh)
            .await
            .unwrap();
        assert!(
            ctx.contains(&digest_line(&lobby_label(), 1, &w2, p.seq, p.seq)),
            "{ctx}"
        );
        assert!(ctx_of(&f, &f.worker).await.is_none());
    }

    #[test]
    fn digest_line_counts_toward_the_cap() {
        let loc = crewd_collab::Locale::Zh;
        let mut b = block(
            "a",
            &[
                ("2026-01-01T00:00:02Z", 2),
                ("2026-01-01T00:00:03Z", 3),
                ("2026-01-01T00:00:04Z", 4),
            ],
        );
        for l in &mut b.lines {
            l.text = "很长的说明".repeat(20);
        }
        let blocks = vec![b];
        let plain = runs_of(&blocks);
        // A folded message before them: announced with what is kept.
        let mut runs = plain.clone();
        runs[0].items.insert(
            0,
            Item {
                seq: 1,
                at: "2026-01-01T00:00:01Z".into(),
                folded: Some("示例发言者".into()),
            },
        );
        let without_digest = units(&render(loc, &blocks, &plain, &[3], None));
        let fitted = fit(loc, blocks.clone(), &runs, without_digest);
        // Item 0 is the folded one (the oldest, always kept); the rest are full lines.
        let kept = fitted.kept[0] - 1;
        assert!(kept < 3, "the digest line must make room: kept {kept}");
        assert!(units(&fitted.context) <= without_digest);
        assert!(fitted.context.contains("示例发言者"), "{}", fitted.context);
    }

    /// M1 (review): the "at least" note follows the unread window counted before folding.
    #[tokio::test]
    async fn a_full_window_with_folded_messages_still_says_at_least() {
        let f = Fixture::with_config(AgentConfig {
            inject_cap_chars: 3_000,
            ..Default::default()
        })
        .await;
        f.add_session(&f.crew, "s-w2", Role::Worker, "另一个 worker")
            .await;
        let l = lobby(&f).await;
        for i in 0..10 {
            peer_post(
                &f,
                "s-w2",
                &l,
                &format!("P-{i:03}"),
                Some("progress"),
                vec![],
                None,
            )
            .await;
        }
        for i in 0..crewd_collab::cursors::UNREAD_CAP as usize {
            f.human_post(&l, &format!("L-{:03}", i % 1000), vec![])
                .await;
        }
        let first = ctx_of(&f, &f.worker).await.unwrap();
        let (pre, _) = text::get(crewd_collab::Locale::Zh, "inject.omitted_at_least")
            .split_once("{count}")
            .unwrap();
        let (_, mid) = pre.split_once("{channel}").unwrap();
        assert!(first.contains(mid), "{first}");
        assert!(first.contains("至少"), "{first}");
    }

    /// M5 (review): digest lines are cut like full lines; what is left out stays unread for the
    /// next call, every call makes progress, and each digest is announced exactly once.
    #[tokio::test]
    async fn digests_alone_over_the_cap_are_spread_over_calls() {
        let cap_units = 700;
        let f = Fixture::with_config(AgentConfig {
            inject_cap_chars: cap_units,
            ..Default::default()
        })
        .await;
        let long = format!("示例同事{}", "很长的名字".repeat(14));
        f.add_session(&f.crew, "s-w2", Role::Worker, &long).await;
        let cap = f.me(&f.captain).await;
        let mut topics = Vec::new();
        for i in 0..6 {
            let t = port::create_channel(
                &f.state,
                &cap,
                ChannelKind::Topic,
                format!("示例话题{i}"),
                None,
                vec![
                    port::MemberRef::Session(f.worker.clone()),
                    port::MemberRef::Session("s-w2".into()),
                ],
            )
            .await
            .unwrap();
            topics.push(t);
        }
        ctx_of(&f, &f.worker).await; // pin the new channels' cursors
        for t in &topics {
            peer_post(&f, "s-w2", &t.id, "P-进度", Some("progress"), vec![], None).await;
        }
        let ctxs = drain(&f, &f.worker, cap_units).await;
        assert!(ctxs.len() > 1, "all six do not fit at once: {ctxs:?}");
        assert!(
            ctxs.len() <= 6,
            "every call announces at least one: {ctxs:?}"
        );
        let header = port::injection_header(crewd_collab::Locale::Zh);
        let mut seen: Vec<String> = Vec::new();
        for c in &ctxs {
            assert!(c.starts_with(&header), "{c}");
            for line in digest_lines(c) {
                let t = topics
                    .iter()
                    .find(|t| line.starts_with(&format!("{}：", t.name)))
                    .unwrap_or_else(|| panic!("{line}"));
                seen.push(t.name.clone());
            }
        }
        seen.sort();
        let mut want: Vec<String> = topics.iter().map(|t| t.name.clone()).collect();
        want.sort();
        assert_eq!(seen, want, "each digest exactly once");
        for t in &topics {
            assert_eq!(
                port::cursor(&f.state, &f.worker, &t.id).await.unwrap(),
                port::latest_seq(&f.state, &t.id).await.unwrap()
            );
        }
    }
}
