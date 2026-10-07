//! 规则拒绝的稳定码（N1 Task 10 / 裁定 14）：`CollabError::{NotFound, Invalid, Forbidden, Conflict}` 装一个
//! `Refusal { code, args }`，文字不再内联在代码里，而是按语言从 `prompts/refusals.{zh,en}.md` 渲染。
//! RPC 错误带 `data.refusal = "<码>"`，客户端按码本地化；`Display` = 中文（日志、RPC message 的缺省）。
//! 每条文字写「下一步怎么做」；占位符的值只放数据（id、数量、成员名），不放给人看的句子 ——
//! 否则中文会漏进英文文字（`render_and_refusal_code_have_no_prose` 盯着）。
use crate::Locale;
use crate::prompts::{fill, refusal_file};
use std::fmt;

macro_rules! refusal_codes {
    ($($(#[$m:meta])* $v:ident = $s:literal [$($a:literal),*]),+ $(,)?) => {
        /// 拒绝码：变体、线上名（`模块.snake名`）、允许的占位符写在一处；两份 md 都必须有同名的节，
        /// 且只用这些占位符（测试盯着）。
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Code { $($(#[$m])* $v),+ }
        impl Code {
            pub const ALL: &'static [Code] = &[$(Code::$v),+];
            pub fn as_str(self) -> &'static str { match self { $(Code::$v => $s),+ } }
            pub fn args(self) -> &'static [&'static str] { match self { $(Code::$v => &[$($a),*]),+ } }
        }
    };
}

refusal_codes! {
    // ask
    AskIdNotFound = "ask.id_not_found" ["crew", "id"],
    AskBadId = "ask.bad_id" ["input"],
    AskSystemCannotWithdraw = "ask.system_cannot_withdraw" [],
    AskArchiveBlocked = "ask.archive_blocked" ["crew", "ask"],
    // channel
    ChannelNotFound = "channel.not_found" ["channel"],
    ChannelSystemCannotAct = "channel.system_cannot_act" [],
    ChannelDmOneHuman = "channel.dm_one_human" [],
    ChannelCreateKindFixed = "channel.create_kind_fixed" [],
    ChannelNameEmpty = "channel.name_empty" [],
    ChannelNameIsSenderLabel = "channel.name_is_sender_label" ["value"],
    ChannelDmNeedsTwo = "channel.dm_needs_two" [],
    ChannelDmWrongCrew = "channel.dm_wrong_crew" ["owner", "crew"],
    ChannelManageForbidden = "channel.manage_forbidden" [],
    ChannelFixedName = "channel.fixed_name" [],
    ChannelFixedNoArchive = "channel.fixed_no_archive" [],
    ChannelLobbyCannotAdd = "channel.lobby_cannot_add" [],
    ChannelDmCannotGrow = "channel.dm_cannot_grow" [],
    ChannelArchived = "channel.archived" ["channel"],
    ChannelAddMembersMemberOnly = "channel.add_members_member_only" [],
    ChannelNoHumanChannel = "channel.no_human_channel" ["crew", "home"],
    ChannelHumanChannelOtherHome = "channel.human_channel_other_home" ["session", "home", "channel"],
    ChannelLobbyCannotRemove = "channel.lobby_cannot_remove" [],
    ChannelDmCannotLeave = "channel.dm_cannot_leave" [],
    ChannelLeaveForbidden = "channel.leave_forbidden" [],
    ChannelOwnerMustTransfer = "channel.owner_must_transfer" [],
    ChannelNotExplicitMember = "channel.not_explicit_member" [],
    ChannelOwnerFollowsCaptain = "channel.owner_follows_captain" [],
    ChannelTransferOwnerOnly = "channel.transfer_owner_only" [],
    ChannelAlreadyOwner = "channel.already_owner" [],
    ChannelNewOwnerMustBeMember = "channel.new_owner_must_be_member" [],
    ChannelTransferTargetLeft = "channel.transfer_target_left" [],
    ChannelTransferNotForYou = "channel.transfer_not_for_you" [],
    ChannelNoPendingTransfer = "channel.no_pending_transfer" [],
    ChannelTransferWithdrawForbidden = "channel.transfer_withdraw_forbidden" [],
    ChannelForceOwnerForbidden = "channel.force_owner_forbidden" [],
    // crew
    CrewNotFound = "crew.not_found" ["crew"],
    CrewArchived = "crew.archived" ["crew"],
    CrewManagementFixed = "crew.management_fixed" [],
    CrewTitleEmpty = "crew.title_empty" [],
    CrewWorkingDirEmpty = "crew.working_dir_empty" [],
    CrewMoveToFirstLevel = "crew.move_to_first_level" ["crew"],
    CrewCannotParentSelf = "crew.cannot_parent_self" [],
    CrewWouldCycle = "crew.would_cycle" ["parent", "crew"],
    CrewNotDirectChild = "crew.not_direct_child" ["crew", "from"],
    CrewReleaseToRootOtherParents = "crew.release_to_root_other_parents" ["crew", "from"],
    CrewRunningSessions = "crew.running_sessions" ["crew", "running"],
    CrewLiveChildren = "crew.live_children" ["crew", "children"],
    CrewWakesInflight = "crew.wakes_inflight" ["crew", "count"],
    CrewParentArchived = "crew.parent_archived" ["crew", "parents"],
    CrewNotArchived = "crew.not_archived" ["crew"],
    CrewSessionNotInCrew = "crew.session_not_in_crew" ["session", "crew"],
    CrewReorderMismatch = "crew.reorder_mismatch" ["current"],
    CrewNoCaptain = "crew.no_captain" ["crew"],
    // directory
    DirectoryBadAddress = "directory.bad_address" ["address"],
    DirectoryNoSuchCrew = "directory.no_such_crew" ["number"],
    DirectoryCrewArchived = "directory.crew_archived" ["number"],
    DirectoryNoSuchExtension = "directory.no_such_extension" ["number", "ext"],
    // message
    MessageNotFound = "message.not_found" ["message"],
    MessageSeqNotFound = "message.seq_not_found" ["seq"],
    MessageBadTime = "message.bad_time" ["value", "why"],
    MessageSearchEmpty = "message.search_empty" [],
    MessageNotAMember = "message.not_a_member" ["who", "channel", "members"],
    MessageCaptainNotAMember = "message.captain_not_a_member" ["captain", "channel", "members"],
    MessageEmpty = "message.empty" [],
    MessageTooLong = "message.too_long" ["max", "actual"],
    MessageReservedCategory = "message.reserved_category" ["category"],
    MessageAttachmentsUnsupported = "message.attachments_unsupported" [],
    MessageSenderNotMember = "message.sender_not_member" ["session", "channel"],
    MessageMentionNeedsTarget = "message.mention_needs_target" [],
    MessageReplyOtherChannel = "message.reply_other_channel" [],
    MessageIdConflict = "message.id_conflict" [],
    // restart
    RestartNotFound = "restart.not_found" ["id"],
    RestartAlreadyResumed = "restart.already_resumed" [],
    RestartSuperseded = "restart.superseded" ["id"],
    // session
    SessionNotFound = "session.not_found" ["session"],
    SessionBadExecutor = "session.bad_executor" ["executor"],
    SessionTitleTooLong = "session.title_too_long" ["max", "actual"],
    SessionTitleOrBriefRequired = "session.title_or_brief_required" ["max"],
    SessionTitleIsSenderLabel = "session.title_is_sender_label" ["value"],
    SessionDisplayNameIsSenderLabel = "session.display_name_is_sender_label" ["value"],
    SessionCaptainExists = "session.captain_exists" ["crew"],
    // settings
    SettingsBadLocale = "settings.bad_locale" ["value"],
    SettingsBadMaxLive = "settings.bad_max_live" ["value"],
    SettingsMaxLiveMin = "settings.max_live_min" [],
    // todo
    TodoResponseEmpty = "todo.response_empty" [],
    TodoNotFound = "todo.not_found" ["number"],
    // wake
    WakeSourceEmpty = "wake.source_empty" [],
    WakeNotFound = "wake.not_found" ["wake"],
    WakeBadTransition = "wake.bad_transition" ["wake", "from", "to"],
    WakeLatchBadScope = "wake.latch_bad_scope" ["scope"],
    WakeLatchRowUnreadable = "wake.latch_row_unreadable" ["scope", "id"],
    // listen
    ListenUntilInPast = "listen.until_in_past" [],
    ListenNotReadable = "listen.not_readable" ["channel"],
}

/// 一次规则拒绝：码 + 填进文字的值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: Code,
    pub args: Vec<(&'static str, String)>,
}

impl Refusal {
    pub fn new(code: Code) -> Self {
        Refusal {
            code,
            args: Vec::new(),
        }
    }

    /// 给占位符 `key` 一个值。`key` 必须是这个码声明过的占位符（debug 构建下 assert）。
    pub fn arg(mut self, key: &'static str, value: impl Into<String>) -> Self {
        debug_assert!(
            self.code.args().contains(&key),
            "{} 没有占位符 {{{key}}}",
            self.code.as_str()
        );
        self.args.push((key, value.into()));
        self
    }

    /// 某个占位符的值（测试与调用方断言「数据」用，不必去匹配渲染出来的句子）。
    pub fn value(&self, key: &str) -> Option<&str> {
        self.args
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }

    /// 按语言渲染。值原样拷贝、不被再展开（同 `prompts::fill`）。
    pub fn text(&self, l: Locale) -> String {
        let vars: Vec<(&str, &str)> = self.args.iter().map(|(k, v)| (*k, v.as_str())).collect();
        debug_assert!(
            self.code
                .args()
                .iter()
                .all(|k| vars.iter().any(|(vk, _)| vk == k)),
            "{} 少给了占位符：要 {:?}，给了 {:?}",
            self.code.as_str(),
            self.code.args(),
            self.args.iter().map(|(k, _)| *k).collect::<Vec<_>>()
        );
        fill(refusal_file(l).get(self.code.as_str()), &vars)
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text(Locale::Zh))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Locale;
    use crate::prompts::refusal_file;
    use crewd_proto::MemberRef;

    #[test]
    fn every_code_has_a_section_in_both_locales_with_exactly_its_placeholders() {
        for l in [Locale::Zh, Locale::En] {
            let f = refusal_file(l);
            for c in Code::ALL {
                let mut used = crate::prompts::placeholders(f.get(c.as_str()));
                used.sort();
                used.dedup();
                let mut want: Vec<&str> = c.args().to_vec();
                want.sort();
                assert_eq!(used, want, "{l:?} {}", c.as_str());
            }
            assert_eq!(
                f.section_names().len(),
                Code::ALL.len(),
                "{l:?}：md 里不能有没有码的节"
            );
        }
    }

    #[test]
    fn a_dm_refuses_new_members_with_a_stable_code_and_text_in_both_languages() {
        let mut s = crate::testkit::store();
        let (dm, _) = crate::testkit::ok(&mut s, |tx| {
            crate::channels::ensure_dm(
                tx,
                "crew-a",
                &MemberRef::session("w-a1"),
                &MemberRef::session("w-a2"),
            )
        });
        let e = crate::testkit::err(&mut s, |tx| {
            crate::channels::add_members(
                tx,
                &crate::Sender::Session("w-a1".into()),
                &dm.id,
                &[MemberRef::session("cap-a")],
            )
        });
        let r = e.refusal().expect("规则拒绝带码");
        assert_eq!(r.code, Code::ChannelDmCannotGrow);
        assert!(
            !r.text(Locale::En)
                .chars()
                .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
            "英文文字里没有中文"
        );
        assert_eq!(e.to_string(), r.text(Locale::Zh));
    }

    fn is_cjk(c: char) -> bool {
        ('\u{4e00}'..='\u{9fff}').contains(&c)
            || ('\u{3000}'..='\u{303f}').contains(&c)
            || ('\u{ff00}'..='\u{ffef}').contains(&c)
    }

    #[test]
    fn english_text_has_no_chinese_whatever_the_args() {
        for c in Code::ALL {
            let mut r = Refusal::new(*c);
            for k in c.args() {
                r = r.arg(k, "x");
            }
            let en = r.text(Locale::En);
            assert!(!en.chars().any(is_cjk), "{}：{en}", c.as_str());
            assert!(!en.contains('{'), "{}：有没填的占位符 {en}", c.as_str());
            let zh = r.text(Locale::Zh);
            assert!(!zh.contains('{'), "{}：有没填的占位符 {zh}", c.as_str());
        }
    }

    /// 去掉源码里的注释（`//…`、`/* … */`），**只在字符串 / 字符字面量之外**：字符串里的 `//`（如 URL 或中文里夹的
    /// `// `）不是注释，不能把后面的中文一起吞掉；保留行结构（行号不变）。处理普通字符串（含转义、跨行）、
    /// 原始字符串 `r#"…"#`、字符字面量 `'"'`（与生命周期 `'a` 区分）。
    fn strip_comments(src: &str) -> String {
        let c: Vec<char> = src.chars().collect();
        let mut out = String::with_capacity(src.len());
        let mut i = 0;
        while i < c.len() {
            match c[i] {
                '/' if c.get(i + 1) == Some(&'/') => {
                    while i < c.len() && c[i] != '\n' {
                        i += 1;
                    }
                }
                '/' if c.get(i + 1) == Some(&'*') => {
                    i += 2;
                    while i < c.len() && !(c[i] == '*' && c.get(i + 1) == Some(&'/')) {
                        if c[i] == '\n' {
                            out.push('\n');
                        }
                        i += 1;
                    }
                    i += 2;
                }
                'r' if matches!(c.get(i + 1), Some('"') | Some('#'))
                    && (i == 0 || !(c[i - 1].is_alphanumeric() || c[i - 1] == '_')) =>
                {
                    let mut j = i + 1;
                    let mut hashes = 0;
                    while c.get(j) == Some(&'#') {
                        hashes += 1;
                        j += 1;
                    }
                    if c.get(j) != Some(&'"') {
                        out.push(c[i]);
                        i += 1;
                        continue;
                    }
                    let close: Vec<char> = std::iter::once('"')
                        .chain(std::iter::repeat_n('#', hashes))
                        .collect();
                    let start = i;
                    i = j + 1;
                    while i < c.len() && c[i..].get(..close.len()) != Some(&close[..]) {
                        i += 1;
                    }
                    i = (i + close.len()).min(c.len());
                    out.extend(&c[start..i]);
                }
                '"' => {
                    let start = i;
                    i += 1;
                    while i < c.len() && c[i] != '"' {
                        if c[i] == '\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                    i = (i + 1).min(c.len());
                    out.extend(&c[start..i]);
                }
                '\'' => {
                    // 字符字面量：'x' 或 '\…'；否则是生命周期，原样放行。
                    let lit_end = if c.get(i + 1) == Some(&'\\') {
                        (i + 2..c.len().min(i + 12)).find(|&k| c[k] == '\'')
                    } else if c.get(i + 2) == Some(&'\'') {
                        Some(i + 2)
                    } else {
                        None
                    };
                    match lit_end {
                        Some(e) => {
                            out.extend(&c[i..=e]);
                            i = e + 1;
                        }
                        None => {
                            out.push('\'');
                            i += 1;
                        }
                    }
                }
                ch => {
                    out.push(ch);
                    i += 1;
                }
            }
        }
        out
    }

    /// 代码里（非测试、非注释的每一行，不只 `CollabError::` 所在行）不得有中文：规则拒绝的文字在 md 里，
    /// 否则中文会经 `.arg(..)` 的值、`format!` 或别的路径漏进英文文字（D8）。返回（行号，整行去首尾空白）。
    fn cjk_lines(src: &str) -> Vec<(usize, String)> {
        let code = match src.find("#[cfg(test)]\nmod tests") {
            Some(i) => &src[..i],
            None => src,
        };
        strip_comments(code)
            .lines()
            .enumerate()
            .filter(|(_, l)| l.chars().any(is_cjk))
            .map(|(i, l)| (i + 1, l.trim().to_string()))
            .collect()
    }

    /// 只进日志 / panic / 内部错误、不会成为规则拒绝的中文诊断（开发者看的）。**整行**（去首尾空白、去注释后）
    /// 逐条列出，不是子串：同一行里再夹别的中文就不在表里，照样红。表里的每一条必须还在源码里（下面的测试查），
    /// 免得表自己烂掉。
    const LOG_ONLY: &[(&str, &str)] = &[
        (
            "crews.rs",
            r#""机组 {crew} 不是 home，往上也走不到一级机组（上级成环或与总机组断开）：数据不一致""#,
        ),
        (
            "crews.rs",
            r#"debug_assert_eq!(n, 1, "put_last 要求边已存在");"#,
        ),
        (
            "prompts.rs",
            r#"None => panic!("prompts/{}.md 缺少一节：## {section}", self.name),"#,
        ),
        (
            "prompts.rs",
            r#"return Err(format!("prompts/{name}.md 的 ## {key} 是空的"));"#,
        ),
        (
            "prompts.rs",
            r#"return Err(format!("prompts/{name}.md 的 ## {key} 重复"));"#,
        ),
        (
            "prompts.rs",
            r#"return Err(format!("prompts/{name}.md 有一个没有名字的 ##"));"#,
        ),
        (
            "asks.rs",
            r#"let a = book.ask(w.ask).expect("human_waits 只列账本里的问");"#,
        ),
        ("refusal.rs", r#""{} 没有占位符 {{{key}}}","#),
        ("refusal.rs", r#""{} 少给了占位符：要 {:?}，给了 {:?}","#),
        ("channels.rs", r#""群 {id} 的 kind = {kind:?}""#),
    ];

    /// 一行（已去注释、去首尾空白）能不能放行：必须与表里该文件的某一条整行相等。
    fn log_only_index(file: &str, line: &str) -> Option<usize> {
        LOG_ONLY.iter().position(|(f, l)| *f == file && *l == line)
    }

    #[test]
    fn the_prose_scanner_catches_chinese_smuggled_through_an_arg() {
        let bad = "return Err(not_a_member(conn, &ch.id, &format!(\"机长 {cap}\"))?);";
        assert_eq!(cjk_lines(bad).len(), 1);
        let ok =
            "// 机长 注释可以有中文\nlet x = 1;\n#[cfg(test)]\nmod tests {\n let y = \"机长\";\n}";
        assert!(cjk_lines(ok).is_empty());
    }

    /// 盲点探针 1：字符串里的 `//` 不是注释；它后面的中文照样要被抓。行尾真注释里的中文不算。
    #[test]
    fn the_scanner_only_strips_comments_outside_string_literals() {
        let hidden = "let u = \"见 http://x // 机长 {cap}\";";
        assert_eq!(cjk_lines(hidden).len(), 1, "字符串里的 // 之后的中文要抓");
        let hidden_raw = "let u = r#\"a // 机长\"#;";
        assert_eq!(cjk_lines(hidden_raw).len(), 1, "原始字符串同理");
        let hidden_char = "let q = '\"'; let u = \"// 机长\";";
        assert_eq!(
            cjk_lines(hidden_char).len(),
            1,
            "字符字面量 '\"' 不能把后面当成字符串起点"
        );
        let comment = "let x = 1; // 机长 是注释\nlet y = 2; /* 机长 */ let z = 3;\n/* 多行\n机长 */\nlet w = 4;";
        assert!(cjk_lines(comment).is_empty(), "{:?}", cjk_lines(comment));
        let lifetime = "fn f<'a>(x: &'a str) -> &'a str { x } // 机长";
        assert!(cjk_lines(lifetime).is_empty());
    }

    /// 盲点探针 2：LOG_ONLY 按整行匹配；放行行里再夹别的中文，不在表里，照样红。
    #[test]
    fn log_only_matches_the_whole_line_not_a_substring() {
        let (file, line) = LOG_ONLY[0];
        assert_eq!(log_only_index(file, line), Some(0));
        let smuggled = format!("{line} + \"机长 {{cap}}\"");
        assert_eq!(log_only_index(file, &smuggled), None);
        assert_eq!(log_only_index("messages.rs", line), None, "别的文件不借用");
    }

    #[test]
    fn render_and_refusal_code_have_no_prose() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut seen = 0;
        let mut used = vec![false; LOG_ONLY.len()];
        let mut hits = Vec::new();
        for e in std::fs::read_dir(&dir).unwrap() {
            let path = e.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if path.extension().is_none_or(|x| x != "rs") || name == "testkit.rs" {
                continue;
            }
            seen += 1;
            for (n, l) in cjk_lines(&std::fs::read_to_string(&path).unwrap()) {
                match log_only_index(&name, &l) {
                    Some(i) => used[i] = true,
                    None => hits.push(format!("{name}:{n}: {l}")),
                }
            }
        }
        for (i, (f, s)) in LOG_ONLY.iter().enumerate() {
            assert!(
                used[i],
                "LOG_ONLY 里的 {f}「{s}」已经不在源码里了（或那一行改了），更新或删掉这一条"
            );
        }
        assert!(seen >= 15, "只扫到 {seen} 个源文件，目录读错了？");
        assert!(
            hits.is_empty(),
            "collab 代码里出现了中文字面量（规则拒绝的文字放进 prompts/refusals.*.md，值里只放数据）：\n{}",
            hits.join("\n")
        );
    }
}
