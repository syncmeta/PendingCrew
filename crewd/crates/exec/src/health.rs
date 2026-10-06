//! 会话健康：进程还活着，但 agent 已经干不了活（Swift `SessionHealth` /
//! `SessionLaunchProbe` 的移植）。只报能可靠拿到的事实，不猜。
//!
//! - 额度：PTY 字节尾窗里的额度短语（每次武装只报一次）。
//! - 限流菜单：尾窗里的 `/rate-limit-options` 菜单 → 调用方自动答「1」（30s 冷却）。
//! - 恢复：额度类 health 置上**之后**开始的一段连续干活满 6s = 已恢复。
//! - 认证：读同一会话的 claude 结构化日志（`projects/*/<id>.jsonl`），不看 PTY 文案。
//! - 拉起自检：25s 内一个字节都没有 / 起来即死 / 根本没起来。
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crewd_term::{AnsiPlainTextTail, RATE_LIMIT_MENU_PHRASES};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthKind {
    /// 未登录 / 登录失效。
    AuthRequired,
    /// 额度到顶 / 余额不足。
    UsageLimit,
    /// 卡在限流菜单（已自动答「等重置」）。
    RateLimited,
    /// 拉起失败（见 `launch_verdict`）。
    LaunchFailed,
    /// 开场任务没送进输入框。
    BriefUndelivered,
    /// 运行中的一条指令没送进输入框。
    PromptUndelivered,
    /// codex：回合失败（协议里的失败回合 / 不再重试的错误），不能当成空闲。
    TurnFailed,
    /// codex：本机 CLI 太旧，服务端要求新版本。
    VersionIncompatible,
}

impl HealthKind {
    /// health 那一格的档位：拉起失败 > 额度 / 限流 > 认证 / 版本不兼容 > 投递报告 / 回合失败。
    /// 低档不能盖掉高档（额度类只由它自己的恢复路径清掉：认证探测的来回既不能盖掉它，也不能借
    /// 「清掉认证」把它清掉）。claude 与 codex 两个核心共用这一张表。
    pub fn rank(self) -> u8 {
        match self {
            Self::LaunchFailed => 4,
            Self::UsageLimit | Self::RateLimited => 3,
            Self::AuthRequired | Self::VersionIncompatible => 2,
            Self::BriefUndelivered | Self::PromptUndelivered | Self::TurnFailed => 1,
        }
    }

    /// 额度类：退出原因「撞额度」与额度重置唤醒都认这组。
    pub fn is_quota(self) -> bool {
        matches!(self, Self::UsageLimit | Self::RateLimited)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Health {
    pub kind: HealthKind,
    /// 人话说明 + 下一步。
    pub detail: String,
    /// 只有拉起自检写这项。
    pub launch_verdict: Option<LaunchVerdict>,
}

impl Health {
    pub fn new(kind: HealthKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
            launch_verdict: None,
        }
    }
}

/// 额度短语（小写子串）。
pub const QUOTA_PHRASES: [&str; 6] = [
    "you've reached your",
    "you\u{2019}ve reached your",
    "usage credit limit reached",
    "out of extra usage",
    "credit balance is too low",
    "credit balance too low",
];

/// 额度扫描器：命中一次后哑掉，直到 [`QuotaScanner::rearm`]。
#[derive(Debug, Default)]
pub struct QuotaScanner {
    tail: AnsiPlainTextTail,
    fired: bool,
}

impl QuotaScanner {
    /// 喂 PTY 原始字节；新命中时返回 `UsageLimit`。
    pub fn feed(&mut self, bytes: &[u8]) -> Option<Health> {
        if self.fired || !self.tail.feed(bytes) {
            return None;
        }
        let hit = QUOTA_PHRASES.iter().any(|p| {
            self.tail
                .contains_lowered_ascii(&AnsiPlainTextTail::lowered_needle(p))
        });
        if !hit {
            return None;
        }
        self.fired = true;
        Some(Health::new(
            HealthKind::UsageLimit,
            "Claude Code 额度受限（usage limit / 余额不足）——等额度窗口重置或检查订阅后再继续。",
        ))
    }

    /// 重新武装（额度恢复后），下个限额窗再撞墙能再报。尾窗一并清掉，免得旧短语立刻再命中。
    pub fn rearm(&mut self) {
        self.fired = false;
        self.tail.clear();
    }
}

/// 限流菜单的自动应答冷却。
pub const RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(30);

/// 限流菜单检测：命中且不在冷却期 → `true`（调用方发「1」、隔 250ms 发回车）。
/// 命中后清空尾窗：按键没生效、菜单还在重绘的话，冷却期满会再次命中 = 内建重试。
#[derive(Debug)]
pub struct RateLimitScanner {
    tail: AnsiPlainTextTail,
    cooldown: Duration,
    last_fired: Option<Instant>,
}

impl Default for RateLimitScanner {
    fn default() -> Self {
        Self::with_cooldown(RATE_LIMIT_COOLDOWN)
    }
}

impl RateLimitScanner {
    pub fn with_cooldown(cooldown: Duration) -> Self {
        Self {
            tail: AnsiPlainTextTail::new(),
            cooldown,
            last_fired: None,
        }
    }

    pub fn feed(&mut self, bytes: &[u8], now: Instant) -> bool {
        if !self.tail.feed(bytes) {
            return false;
        }
        let hit = RATE_LIMIT_MENU_PHRASES.iter().any(|p| {
            self.tail
                .contains_lowered_ascii(&AnsiPlainTextTail::lowered_needle(p))
        });
        if !hit {
            return false;
        }
        if self
            .last_fired
            .is_some_and(|last| now.saturating_duration_since(last) < self.cooldown)
        {
            return false;
        }
        self.last_fired = Some(now);
        self.tail.clear();
        true
    }
}

/// 额度类 health 之后连续干活多久算恢复。
pub const QUOTA_WORK_STREAK: Duration = Duration::from_secs(6);

/// 额度类 health 是否已恢复：干活 streak 起点晚于置上时刻，且已持续 `QUOTA_WORK_STREAK`。
pub fn quota_recovered(
    health: Option<&Health>,
    quota_at: Option<Instant>,
    working_since: Option<Instant>,
    now: Instant,
) -> bool {
    let (Some(h), Some(at), Some(since)) = (health, quota_at, working_since) else {
        return false;
    };
    h.kind.is_quota() && since > at && now.saturating_duration_since(since) >= QUOTA_WORK_STREAK
}

/// 结构化日志给出的认证证据。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthState {
    Unknown,
    Authenticated,
    AuthenticationRequired,
}

impl AuthState {
    /// 把认证证据叠到当前 health 上：失败 → `AuthRequired`；其余只清掉旧的 `AuthRequired`。
    pub fn apply(self, health: Option<Health>) -> Option<Health> {
        match self {
            Self::AuthenticationRequired => Some(Health::new(
                HealthKind::AuthRequired,
                "Claude Code 在本 session 报告了认证失败。请检查该 session 的登录或凭据配置。",
            )),
            Self::Authenticated | Self::Unknown => {
                health.filter(|h| h.kind != HealthKind::AuthRequired)
            }
        }
    }
}

/// 认证探测多久跑一次。
pub const AUTH_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// 读 `projects_dir/*/<session_id>.jsonl` 末尾一段，看这次启动以来最后一条
/// assistant 记录是认证失败还是正常回答。
#[derive(Clone, Debug)]
pub struct AuthProbe {
    pub session_id: String,
    pub projects_dir: PathBuf,
    /// 只认这个时刻之后的记录（本次启动）。
    pub since: SystemTime,
}

impl AuthProbe {
    /// 最多读日志末尾这么多字节。
    pub const MAX_BYTES: u64 = 256 * 1024;

    /// claude 的 projects 目录：`CLAUDE_CONFIG_DIR`（相对路径按 `workdir` 解析）或 `~/.claude`。
    pub fn projects_dir_for(
        config_dir: Option<&str>,
        home: Option<&str>,
        workdir: &Path,
    ) -> Option<PathBuf> {
        let base = match config_dir.filter(|d| !d.is_empty()) {
            Some(d) => workdir.join(d),
            None => Path::new(home.filter(|h| !h.is_empty())?).join(".claude"),
        };
        Some(base.join("projects"))
    }

    /// 读不到、对不上、有歧义一律 `Unknown`——读不到日志不等于没登录。
    pub fn read(&self) -> AuthState {
        let id = &self.session_id;
        if id.is_empty() || id.contains(['/', '\\', '\0']) {
            return AuthState::Unknown;
        }
        let Ok(entries) = std::fs::read_dir(&self.projects_dir) else {
            return AuthState::Unknown;
        };
        let candidates: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .map(|e| e.path().join(format!("{id}.jsonl")))
            .filter(|p| p.exists())
            .collect();
        let [file] = candidates.as_slice() else {
            return AuthState::Unknown;
        };
        match read_tail(file, Self::MAX_BYTES) {
            Some(data) => auth_state_in(&data, id, self.since),
            None => AuthState::Unknown,
        }
    }
}

/// 读文件末尾 `max` 字节；从中间开始读时丢掉第一条（可能是半截）记录。
fn read_tail(path: &Path, max: u64) -> Option<Vec<u8>> {
    let mut f = std::fs::File::open(path).ok()?;
    let size = f.metadata().ok()?.len();
    let offset = size.saturating_sub(max);
    f.seek(SeekFrom::Start(offset)).ok()?;
    let mut data = Vec::new();
    f.take(max).read_to_end(&mut data).ok()?;
    if offset > 0 {
        let newline = data.iter().position(|&b| b == b'\n')?;
        data.drain(..=newline);
    }
    Some(data)
}

/// 在一段 JSONL 里找本会话、本次启动以来最新的那条 assistant 记录并判定。
/// 最后一段没有换行结尾的（可能写到一半）不算。
pub fn auth_state_in(data: &[u8], session_id: &str, since: SystemTime) -> AuthState {
    let mut state = AuthState::Unknown;
    let mut newest = chrono::DateTime::<chrono::Utc>::from(since);
    let mut lines: Vec<&[u8]> = data.split(|&b| b == b'\n').collect();
    lines.pop();
    for line in lines {
        let Ok(obj) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        let owned = obj["sessionId"] == session_id
            && obj["type"] == "assistant"
            && obj["isSidechain"] != Value::Bool(true);
        let Some(ts) = obj["timestamp"]
            .as_str()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        else {
            continue;
        };
        let ts = ts.with_timezone(&chrono::Utc);
        if !owned || ts < newest {
            continue;
        }
        newest = ts;
        let api_error = obj["isApiErrorMessage"] == Value::Bool(true);
        let msg = &obj["message"];
        state = if api_error && obj["error"] == "authentication_failed" {
            AuthState::AuthenticationRequired
        } else if !api_error
            && obj.get("error").is_none()
            && msg["role"] == "assistant"
            && msg["model"]
                .as_str()
                .is_some_and(|m| !m.is_empty() && !m.starts_with('<'))
            && msg["id"].as_str().is_some_and(|i| i.starts_with("msg_"))
            && msg["content"].is_array()
        {
            AuthState::Authenticated
        } else {
            AuthState::Unknown
        };
    }
    state
}

/// 拉起自检的观察窗与轮询间隔。
pub const FIRST_OUTPUT_DEADLINE: Duration = Duration::from_secs(25);
pub const LAUNCH_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchVerdict {
    /// 观察窗内、没有坏消息——继续看。
    Pending,
    /// 根本没起来。
    SpawnFailed,
    /// 起过、没吐一个字就没了。
    DiedSilently,
    /// 活着，但过了期限一个字节都没吐。
    Stalled,
    /// 见过输出——正常，自检收工。
    Alive,
}

impl LaunchVerdict {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::SpawnFailed | Self::DiedSilently | Self::Stalled)
    }
}

/// 拉起自检判定（纯函数）。见过输出优先于一切（除了没 spawn 起来）。
pub fn launch_verdict(
    spawned: bool,
    process_alive: bool,
    ever_alive: bool,
    saw_output: bool,
    elapsed: Duration,
    deadline: Duration,
) -> LaunchVerdict {
    if !spawned {
        return LaunchVerdict::SpawnFailed;
    }
    if saw_output {
        return LaunchVerdict::Alive;
    }
    if process_alive {
        return if elapsed >= deadline {
            LaunchVerdict::Stalled
        } else {
            LaunchVerdict::Pending
        };
    }
    if ever_alive {
        return LaunchVerdict::DiedSilently;
    }
    if elapsed >= deadline {
        LaunchVerdict::SpawnFailed
    } else {
        LaunchVerdict::Pending
    }
}

/// 失败裁决 → health（`Alive`/`Pending` 返回 `None`）。`underlying` 是附带的原因。
pub fn launch_failure(
    verdict: LaunchVerdict,
    deadline: Duration,
    underlying: Option<&str>,
) -> Option<Health> {
    let tail = underlying.map(|u| format!("（{u}）")).unwrap_or_default();
    let detail = match verdict {
        LaunchVerdict::SpawnFailed => format!(
            "Claude Code 子进程没能启动{tail}——这个 session 从头到尾没跑起来，活等于没派出去，请改派或排查后重起。"
        ),
        LaunchVerdict::DiedSilently => format!(
            "Claude Code 子进程启动后立刻退出{tail}——常见于额度耗尽 / 未登录 / 工作目录不可用，请核实后重起。"
        ),
        LaunchVerdict::Stalled => format!(
            "Claude Code 起来了但 {} 秒内一个字都没输出{tail}——半死状态（进程在、干不了活），请改派或重起。",
            deadline.as_secs()
        ),
        LaunchVerdict::Alive | LaunchVerdict::Pending => return None,
    };
    Some(Health {
        kind: HealthKind::LaunchFailed,
        detail,
        launch_verdict: Some(verdict),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn quota_phrase_fires_once_until_rearmed() {
        let mut q = QuotaScanner::default();
        assert_eq!(q.feed(b"working on it...\r\n"), None);
        let h = q
            .feed(b"\x1b[31mYou've reached\x1b[0m your usage limit")
            .unwrap();
        assert_eq!(h.kind, HealthKind::UsageLimit);
        assert_eq!(
            q.feed(b"You've reached your usage limit"),
            None,
            "哑掉直到重新武装"
        );
        q.rearm();
        assert_eq!(
            q.feed(b"still fine"),
            None,
            "重新武装时尾窗清空，旧短语不再命中"
        );
        assert!(
            q.feed("You\u{2019}ve reached your limit".as_bytes())
                .is_some()
        );
    }

    #[test]
    fn quota_phrase_split_across_chunks_matches() {
        let mut q = QuotaScanner::default();
        assert_eq!(q.feed(b"Credit bal"), None);
        assert!(q.feed(b"ance is too low").is_some());
    }

    #[test]
    fn login_documentation_quoted_in_output_is_not_a_health_event() {
        let mut q = QuotaScanner::default();
        assert_eq!(
            q.feed(b"static let authPhrases = [\"run /login\", \"not logged in\"]"),
            None
        );
    }

    #[test]
    fn rate_limit_menu_fires_with_cooldown() {
        let t0 = Instant::now();
        let mut r = RateLimitScanner::default();
        assert!(!r.feed(b"ordinary output", t0));
        assert!(r.feed(
            b"What do you want to do?\r\n\x1b[1m1. Stop and wait for limit to reset\x1b[0m",
            t0
        ));
        assert!(
            !r.feed(b"1. Stop and wait for limit to reset", t0 + secs(10)),
            "冷却期内不重复"
        );
        assert!(
            r.feed(b"/rate-limit-options", t0 + secs(31)),
            "冷却期满再命中 = 重试"
        );
    }

    #[test]
    fn quota_recovery_needs_a_fresh_long_enough_streak() {
        let t0 = Instant::now();
        let quota = Health::new(HealthKind::RateLimited, "x");
        let other = Health::new(HealthKind::PromptUndelivered, "x");
        assert!(quota_recovered(
            Some(&quota),
            Some(t0),
            Some(t0 + secs(1)),
            t0 + secs(7)
        ));
        assert!(!quota_recovered(
            Some(&quota),
            Some(t0),
            Some(t0 + secs(1)),
            t0 + secs(6)
        ));
        assert!(
            !quota_recovered(Some(&quota), Some(t0), Some(t0), t0 + secs(60)),
            "streak 早于撞墙不算"
        );
        assert!(!quota_recovered(
            Some(&quota),
            Some(t0),
            None,
            t0 + secs(60)
        ));
        assert!(!quota_recovered(
            Some(&other),
            Some(t0),
            Some(t0 + secs(1)),
            t0 + secs(60)
        ));
    }

    fn since() -> SystemTime {
        SystemTime::UNIX_EPOCH + secs(1_800_000_000)
    }

    fn record(failed: bool, offset_secs: i64) -> Value {
        let ts =
            chrono::DateTime::<chrono::Utc>::from(since()) + chrono::Duration::seconds(offset_secs);
        let mut r = serde_json::json!({
            "sessionId": "owned", "type": "assistant", "isSidechain": false,
            "timestamp": ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "message": {"role": "assistant", "id": "msg_real", "model": "claude-model",
                        "content": [{"type": "text", "text": "run /login is quoted"}]},
        });
        if failed {
            r["isApiErrorMessage"] = Value::Bool(true);
            r["error"] = "authentication_failed".into();
        }
        r
    }

    fn jsonl(records: &[Value]) -> Vec<u8> {
        let mut out = Vec::new();
        for r in records {
            out.extend(serde_json::to_vec(r).unwrap());
            out.push(b'\n');
        }
        out
    }

    fn state(records: &[Value]) -> AuthState {
        auth_state_in(&jsonl(records), "owned", since())
    }

    #[test]
    fn auth_failure_and_recovery_follow_the_newest_owned_record() {
        assert_eq!(state(&[]), AuthState::Unknown);
        assert_eq!(state(&[record(false, 1)]), AuthState::Authenticated);
        assert_eq!(state(&[record(true, 1)]), AuthState::AuthenticationRequired);
        assert_eq!(
            state(&[record(true, 1), record(false, 2)]),
            AuthState::Authenticated
        );
        assert_eq!(
            state(&[record(true, 1), record(false, 2), record(true, 3)]),
            AuthState::AuthenticationRequired
        );
    }

    #[test]
    fn auth_evidence_must_be_current_owned_and_from_the_runner() {
        let mut other = record(true, 1);
        other["sessionId"] = "other".into();
        let mut sidechain = record(true, 1);
        sidechain["isSidechain"] = Value::Bool(true);
        let mut user = record(true, 1);
        user["type"] = "user".into();
        let mut synthetic = record(false, 1);
        synthetic["message"]["model"] = "<synthetic>".into();
        for r in [other, sidechain, user, synthetic, record(true, -10)] {
            assert_eq!(state(std::slice::from_ref(&r)), AuthState::Unknown, "{r}");
        }
        let mut partial = jsonl(&[record(true, 1)]);
        partial.pop();
        assert_eq!(
            auth_state_in(&partial, "owned", since()),
            AuthState::Unknown,
            "半截记录不算"
        );
        assert_eq!(
            auth_state_in(b"broken\n", "owned", since()),
            AuthState::Unknown
        );
    }

    #[test]
    fn auth_state_only_touches_auth_health() {
        let quota = Health::new(HealthKind::UsageLimit, "q");
        assert_eq!(
            AuthState::Unknown.apply(Some(quota.clone())),
            Some(quota.clone())
        );
        assert_eq!(AuthState::Unknown.apply(None), None);
        let auth = AuthState::AuthenticationRequired.apply(None).unwrap();
        assert_eq!(auth.kind, HealthKind::AuthRequired);
        assert_eq!(AuthState::Authenticated.apply(Some(auth)), None);
    }

    #[test]
    fn probe_reads_disk_and_refuses_ambiguity() {
        let root = tempfile::tempdir().unwrap();
        let probe = AuthProbe {
            session_id: "owned".into(),
            projects_dir: root.path().to_path_buf(),
            since: since(),
        };
        assert_eq!(probe.read(), AuthState::Unknown);
        for (name, expect) in [
            ("a", AuthState::AuthenticationRequired),
            ("b", AuthState::Unknown),
        ] {
            let d = root.path().join(name);
            std::fs::create_dir(&d).unwrap();
            std::fs::write(d.join("owned.jsonl"), jsonl(&[record(true, 1)])).unwrap();
            assert_eq!(probe.read(), expect, "两个目录都有同名日志 = 有歧义");
        }
        let bad = AuthProbe {
            session_id: "../owned".into(),
            ..probe
        };
        assert_eq!(bad.read(), AuthState::Unknown);
    }

    #[test]
    fn probe_reads_only_the_bounded_tail() {
        let root = tempfile::tempdir().unwrap();
        let d = root.path().join("p");
        std::fs::create_dir(&d).unwrap();
        let mut data = vec![b' '; AuthProbe::MAX_BYTES as usize + 20];
        data.push(b'\n');
        data.extend(jsonl(&[record(true, 1)]));
        std::fs::write(d.join("owned.jsonl"), data).unwrap();
        let probe = AuthProbe {
            session_id: "owned".into(),
            projects_dir: root.path().to_path_buf(),
            since: since(),
        };
        assert_eq!(probe.read(), AuthState::AuthenticationRequired);
        std::fs::remove_file(d.join("owned.jsonl")).unwrap();
        std::fs::create_dir(d.join("owned.jsonl")).unwrap();
        assert_eq!(probe.read(), AuthState::Unknown, "读不了日志不等于没登录");
    }

    #[test]
    fn projects_dir_resolution() {
        let wd = Path::new("/work");
        assert_eq!(
            AuthProbe::projects_dir_for(None, Some("/home/u"), wd),
            Some(PathBuf::from("/home/u/.claude/projects"))
        );
        assert_eq!(
            AuthProbe::projects_dir_for(Some("cfg"), Some("/home/u"), wd),
            Some(PathBuf::from("/work/cfg/projects"))
        );
        assert_eq!(
            AuthProbe::projects_dir_for(Some("/abs"), None, wd),
            Some(PathBuf::from("/abs/projects"))
        );
        assert_eq!(AuthProbe::projects_dir_for(None, None, wd), None);
    }

    #[test]
    fn launch_verdicts() {
        let d = FIRST_OUTPUT_DEADLINE;
        use LaunchVerdict::*;
        assert_eq!(
            launch_verdict(false, false, false, true, secs(0), d),
            SpawnFailed
        );
        assert_eq!(
            launch_verdict(true, false, true, false, secs(1), d),
            DiedSilently
        );
        assert_eq!(
            launch_verdict(true, false, false, false, secs(1), d),
            Pending,
            "还没生 ≠ 死了"
        );
        assert_eq!(launch_verdict(true, false, false, false, d, d), SpawnFailed);
        assert_eq!(
            launch_verdict(true, true, true, false, secs(24), d),
            Pending
        );
        assert_eq!(launch_verdict(true, true, true, false, d, d), Stalled);
        assert_eq!(
            launch_verdict(true, false, true, true, secs(1), d),
            Alive,
            "见过输出就归退出回调管"
        );
        assert!(Stalled.is_terminal() && DiedSilently.is_terminal() && SpawnFailed.is_terminal());
        assert!(!Pending.is_terminal() && !Alive.is_terminal());
    }

    #[test]
    fn launch_failure_detail() {
        let h = launch_failure(LaunchVerdict::DiedSilently, secs(25), Some("exit 1")).unwrap();
        assert_eq!(h.kind, HealthKind::LaunchFailed);
        assert_eq!(h.launch_verdict, Some(LaunchVerdict::DiedSilently));
        assert!(h.detail.contains("（exit 1）"), "{}", h.detail);
        assert!(
            launch_failure(LaunchVerdict::Stalled, secs(25), None)
                .unwrap()
                .detail
                .contains("25 秒")
        );
        assert_eq!(launch_failure(LaunchVerdict::Alive, secs(25), None), None);
    }
}
