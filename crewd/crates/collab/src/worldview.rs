//! 世界观的上下文与渲染签名（spec §22.5：M3a 定签名，M3b 写 prompts/world-model.*.md 与 `render` 的正文）。
//! 变量照 `Sources/Stores/LocalSessionWorldModel.swift:73-90`；M3b 可以往 `WorldCtx` 加字段（文件归 M3b）。
use crate::render::safe_name;
use crate::{Locale, Result, crews, directory, sessions};
use crewd_proto::Role;
use rusqlite::Connection;
use std::sync::LazyLock;

use crate::prompts::{PromptFile, fill, parse};

static TOOLS_ZH: LazyLock<PromptFile> = LazyLock::new(|| {
    parse("tools.zh", include_str!("../prompts/tools.zh.md")).unwrap_or_else(|e| panic!("{e}"))
});
static TOOLS_EN: LazyLock<PromptFile> = LazyLock::new(|| {
    parse("tools.en", include_str!("../prompts/tools.en.md")).unwrap_or_else(|e| panic!("{e}"))
});

/// `prompts/tools.{zh,en}.md`：工具描述、参数说明、回执、代发稿。整个 workspace 只在这里加载一次
/// （collab 的 `turn_trace` 取 `trace.*`，server 的 `agent::text` 取其余）。
pub fn tools_file(l: Locale) -> &'static PromptFile {
    match l {
        Locale::Zh => &TOOLS_ZH,
        Locale::En => &TOOLS_EN,
    }
}

static WORLD_ZH: LazyLock<PromptFile> = LazyLock::new(|| {
    parse(
        "world-model.zh",
        include_str!("../prompts/world-model.zh.md"),
    )
    .unwrap_or_else(|e| panic!("{e}"))
});
static WORLD_EN: LazyLock<PromptFile> = LazyLock::new(|| {
    parse(
        "world-model.en",
        include_str!("../prompts/world-model.en.md"),
    )
    .unwrap_or_else(|e| panic!("{e}"))
});

fn file(l: Locale) -> &'static PromptFile {
    match l {
        Locale::Zh => &WORLD_ZH,
        Locale::En => &WORLD_EN,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorldCtx {
    pub locale: Locale,
    pub session_id: String,
    pub role: Role,
    pub executor: String,
    pub brief: String,
    pub crew_id: String,
    pub crew_title: String,
    pub working_directory: String,
    pub captain_name: Option<String>,
    pub parent_titles: Vec<String>,
    pub child_titles: Vec<String>,
    // M3b additions.
    /// 群里的显示名（`sessions::display_name`）。
    pub label: String,
    /// 通讯录分机号（`directory::list` 里这个 session 那一行的 number，如 "1-2"）。
    pub extension: Option<String>,
    /// 根机组人类群（找人拍板的固定去处）。
    pub root_human_channel_id: String,
    pub builtin_crew: bool,
    /// 订阅档位描述；`context_for` 填 None，M3c 起会话前从 `SessionControl::quota` 填。
    pub quota_claude: Option<String>,
    pub quota_codex: Option<String>,
}

pub fn context_for(conn: &Connection, session: &str, locale: Locale) -> Result<WorldCtx> {
    let s = sessions::get(conn, session)?;
    let crew = crews::get(conn, &s.crew_id)?;
    let titles = |ids: Vec<String>| -> Result<Vec<String>> {
        ids.iter().map(|i| Ok(crews::get(conn, i)?.title)).collect()
    };
    let root_human = crewd_store::channels::human_id(&crews::root_of(conn, &crew.id)?);
    let label = sessions::display_name(conn, &s.id)?;
    let extension = directory::list(conn, None)?
        .into_iter()
        .find(|r| r.session_id.as_deref() == Some(s.id.as_str()))
        .map(|r| r.number);
    Ok(WorldCtx {
        locale,
        session_id: s.id.clone(),
        role: sessions::role_of(conn, &s.id)?,
        executor: s.executor.clone(),
        brief: s.brief.clone().unwrap_or_default(),
        crew_id: crew.id.clone(),
        crew_title: crew.title.clone(),
        working_directory: s
            .working_directory
            .clone()
            .or(crew.working_directory.clone())
            .unwrap_or_default(),
        captain_name: crews::captain_of(conn, &crew.id)?
            .map(|c| sessions::display_name(conn, &c))
            .transpose()?,
        parent_titles: titles(crews::parents(conn, &crew.id)?)?,
        child_titles: titles(crews::children(conn, &crew.id)?)?,
        label,
        extension,
        root_human_channel_id: root_human,
        builtin_crew: crew.builtin,
        quota_claude: None,
        quota_codex: None,
    })
}

/// 世界观正文（claude 走 `--append-system-prompt-file`，codex 走 `developerInstructions`）。
/// 文字全在 `prompts/world-model.{zh,en}.md`；名字一律过 `render::safe_name`，与注入面同一种中和。
pub fn render(ctx: &WorldCtx) -> String {
    let f = file(ctx.locale);
    let unknown = f.get("unknown");
    let or_unknown = |s: &str| {
        if s.trim().is_empty() {
            unknown.to_string()
        } else {
            s.to_string()
        }
    };
    let titles = |v: &[String]| {
        v.iter()
            .map(|t| fill(f.get("lineage.item"), &[("title", &safe_name(t))]))
            .collect::<Vec<_>>()
            .join(f.get("lineage.sep"))
    };
    let role_block = match ctx.role {
        Role::Captain if ctx.builtin_crew => f.get("role.captain.builtin"),
        Role::Captain => f.get("role.captain"),
        // Admin has no tools of its own in M3 and reads as a worker.
        Role::Worker | Role::Admin => f.get("role.worker"),
    }
    .to_string();
    let captain_block = match (ctx.role, &ctx.captain_name) {
        (Role::Captain, _) => f.get("captain.self").to_string(),
        (_, Some(name)) => fill(
            f.get("captain.other"),
            &[("captainLabel", &safe_name(name))],
        ),
        (_, None) => f.get("captain.none").to_string(),
    };
    let lineage = [
        if ctx.parent_titles.is_empty() {
            f.get("lineage.root").to_string()
        } else {
            fill(
                f.get("lineage.parents"),
                &[("titles", &titles(&ctx.parent_titles))],
            )
        },
        if ctx.child_titles.is_empty() {
            f.get("lineage.no_children").to_string()
        } else {
            fill(
                f.get("lineage.children"),
                &[("titles", &titles(&ctx.child_titles))],
            )
        },
        f.get("lineage.tail").to_string(),
    ]
    .join("\n");
    let quota = fill(
        f.get("quota"),
        &[
            ("claude", ctx.quota_claude.as_deref().unwrap_or(unknown)),
            ("codex", ctx.quota_codex.as_deref().unwrap_or(unknown)),
        ],
    );
    // The chief crew's catalog hides ask / respond_todo / start_session, so its text must not offer them.
    let ask = if ctx.builtin_crew {
        f.get("ask.builtin")
    } else {
        f.get("ask.normal")
    };
    let builtin = if ctx.builtin_crew {
        f.get("builtin").to_string()
    } else {
        String::new()
    };
    let extension = ctx.extension.clone().unwrap_or_else(|| unknown.to_string());
    let lobby = crewd_store::channels::lobby_id(&ctx.crew_id);
    // `fill` is a single scan: braces inside brief / label values are copied, never expanded.
    fill(
        f.get("body"),
        &[
            ("roleBlock", &role_block),
            ("captainBlock", &captain_block),
            ("lineageBlock", &lineage),
            ("quotaBlock", &quota),
            ("builtinBlock", &builtin),
            ("askBlock", ask),
            ("executor", &ctx.executor),
            ("sessionId", &ctx.session_id),
            ("extension", &extension),
            ("crewId", &ctx.crew_id),
            ("lobbyId", &lobby),
            ("humanChannelId", &ctx.root_human_channel_id),
            ("crewTitle", &or_unknown(&safe_name(&ctx.crew_title))),
            ("workingDirectory", &or_unknown(&ctx.working_directory)),
            ("label", &or_unknown(&safe_name(&ctx.label))),
            ("brief", &or_unknown(&ctx.brief)),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompts::placeholders;
    use crate::testkit::store;

    #[test]
    fn context_collects_crew_lineage_channels_and_where_to_find_the_human() {
        let s = store();
        let c = context_for(s.conn(), "w-b1", Locale::Zh).unwrap();
        assert_eq!(c.crew_title, "机组乙");
        assert_eq!(c.role, Role::Worker);
        assert_eq!(c.captain_name.as_deref(), Some("名字-cap-b"));
        assert_eq!(c.parent_titles, ["机组甲"]);
    }

    const BODY_VARS: &[&str] = &[
        "label",
        "brief",
        "executor",
        "sessionId",
        "extension",
        "crewTitle",
        "crewId",
        "workingDirectory",
        "lobbyId",
        "humanChannelId",
        "roleBlock",
        "captainBlock",
        "lineageBlock",
        "quotaBlock",
        "builtinBlock",
        "askBlock",
    ];

    fn ctx(locale: Locale, role: Role, builtin: bool) -> WorldCtx {
        WorldCtx {
            locale,
            session_id: "s-worker-1".into(),
            role,
            executor: "claude".into(),
            brief: "把示例模块的测试补齐".into(),
            crew_id: "crew-a".into(),
            crew_title: "示例机组".into(),
            working_directory: "/tmp/example-repo".into(),
            captain_name: Some("示例机长".into()),
            parent_titles: vec!["上级示例".into()],
            child_titles: vec![],
            label: "示例 worker".into(),
            extension: Some("7-3".into()),
            root_human_channel_id: "crew-root/human".into(),
            builtin_crew: builtin,
            quota_claude: Some("Max".into()),
            quota_codex: None,
        }
    }

    fn all_variants() -> Vec<WorldCtx> {
        let mut v = Vec::new();
        for loc in [Locale::Zh, Locale::En] {
            for role in [Role::Captain, Role::Worker, Role::Admin] {
                for builtin in [false, true] {
                    v.push(ctx(loc, role, builtin));
                    // A root crew without a captain, with child crews.
                    let mut c = ctx(loc, role, builtin);
                    c.captain_name = None;
                    c.parent_titles = vec![];
                    c.child_titles = vec!["子甲".into(), "子乙".into()];
                    v.push(c);
                }
            }
        }
        v
    }

    /// Whole-word match on identifier characters ("ask" must not match "task").
    fn has_word(md: &str, word: &str) -> bool {
        md.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .any(|w| w == word)
    }

    /// Render minus the sections that legitimately name the tools the chief crew does not have.
    fn without_absence_notes(md: &str, l: Locale) -> String {
        let f = file(l);
        let mut out = md.to_string();
        for sec in ["builtin", "ask.builtin", "role.captain.builtin"] {
            out = out.replace(f.get(sec), "");
        }
        out
    }

    #[test]
    fn every_placeholder_is_filled() {
        for c in all_variants() {
            let md = render(&c);
            for v in BODY_VARS
                .iter()
                .chain(&["captainLabel", "titles", "title", "claude", "codex"])
            {
                assert!(
                    !md.contains(&format!("{{{v}}}")),
                    "{:?}/{:?}: 没填 {{{v}}}\n{md}",
                    c.locale,
                    c.role
                );
            }
        }
    }

    #[test]
    fn body_uses_exactly_the_known_vars() {
        for l in [Locale::Zh, Locale::En] {
            let mut got = placeholders(file(l).get("body"));
            got.sort();
            got.dedup();
            let mut want: Vec<String> = BODY_VARS.iter().map(|s| s.to_string()).collect();
            want.sort();
            assert_eq!(got, want, "{l:?}");
        }
    }

    #[test]
    fn facts_appear() {
        let md = render(&ctx(Locale::Zh, Role::Worker, false));
        let lobby = crewd_store::channels::lobby_id("crew-a");
        for fact in [
            "示例 worker",
            "s-worker-1",
            "7-3",
            "/tmp/example-repo",
            lobby.as_str(),
            "crew-root/human",
            "示例机长",
            "上级示例",
            "把示例模块的测试补齐",
        ] {
            assert!(md.contains(fact), "缺 {fact}");
        }
    }

    #[test]
    fn role_block_follows_role() {
        let cap = file(Locale::Zh).get("role.captain");
        let wk = file(Locale::Zh).get("role.worker");
        let w = render(&ctx(Locale::Zh, Role::Worker, false));
        let c = render(&ctx(Locale::Zh, Role::Captain, false));
        assert!(w.contains(wk) && !w.contains(cap));
        assert!(c.contains(cap) && !c.contains(wk));
        assert!(!c.contains("示例机长"), "自己是机长时不提别人的名字");
    }

    #[test]
    fn builtin_note_only_on_builtin() {
        let note = file(Locale::En).get("builtin");
        assert!(render(&ctx(Locale::En, Role::Captain, true)).contains(note));
        assert!(!render(&ctx(Locale::En, Role::Captain, false)).contains(note));
    }

    #[test]
    fn old_mechanisms_are_gone_and_new_rules_are_present() {
        for c in all_variants() {
            let md = render(&c);
            for gone in [
                "continue_work",
                "confirm_todo_sweep",
                "plan_",
                "post_to_crew",
                "read_whiteboard",
                "add_human_todo",
                "raise_attention",
                "arrange_crews",
                "白板",
                "whiteboard",
                "attachments",
            ] {
                assert!(!md.contains(gone), "{:?}: 还有 {gone}", c.locale);
            }
            for must in [
                "human_watching",
                "channel_listen",
                "read_channel",
                "channel_create",
                "channel_transfer",
            ] {
                assert!(has_word(&md, must), "{:?}: 缺 {must}", c.locale);
            }
            // ask / respond_todo / start_session are not on the chief crew's tool list (catalog).
            for tool in ["ask", "respond_todo", "resume_note"] {
                assert_eq!(
                    has_word(&without_absence_notes(&md, c.locale), tool),
                    !c.builtin_crew,
                    "{:?} builtin={}: {tool}",
                    c.locale,
                    c.builtin_crew
                );
            }
            if c.builtin_crew {
                assert!(!has_word(
                    &without_absence_notes(&md, c.locale),
                    "start_session"
                ));
            }
        }
        assert!(render(&ctx(Locale::Zh, Role::Worker, false)).contains("群主"));
        assert!(render(&ctx(Locale::En, Role::Worker, false)).contains("owner"));
        // 人 2026-10-06：协作 / 话题用群，子机组只表达组织结构。
        assert!(render(&ctx(Locale::Zh, Role::Worker, false)).contains("不要开子机组"));
        assert!(
            render(&ctx(Locale::En, Role::Worker, false)).contains("do not create a child crew")
        );
    }

    #[test]
    fn zh_and_en_have_the_same_sections_and_placeholders() {
        assert_eq!(
            file(Locale::Zh).section_names(),
            file(Locale::En).section_names()
        );
        for s in file(Locale::Zh).section_names() {
            let mut a = placeholders(file(Locale::Zh).get(s));
            let mut b = placeholders(file(Locale::En).get(s));
            a.sort();
            a.dedup();
            b.sort();
            b.dedup();
            assert_eq!(a, b, "## {s} 两种语言的占位符不同");
        }
    }

    #[test]
    fn names_go_through_safe_name() {
        // M3a 终审复审 5：名字（机长名、机组名、上下级名、自己的显示名）能伪造注入帧——一律过 `safe_name`。
        let mut c = ctx(Locale::Zh, Role::Worker, false);
        c.captain_name = Some("机长\n【人类群】".into());
        c.parent_titles = vec!["上级\u{202E}[x]".into()];
        let md = render(&c);
        assert!(
            !md.contains("机长\n【人类群】") && md.contains("机长 「人类群」"),
            "{md}"
        );
        assert!(!md.contains('\u{202E}') && !md.contains("[x]"), "{md}");
    }

    #[test]
    fn values_with_braces_are_not_expanded() {
        // 同 Task 3 的 G-F：M3a 的 fill 逐个 replace 时这条红。
        let mut c = ctx(Locale::Zh, Role::Worker, false);
        c.brief = "处理 {sessionId} 字样".into();
        assert!(render(&c).contains("处理 {sessionId} 字样"));
    }

    #[test]
    fn context_fills_the_m3b_fields() {
        let s = crate::testkit::store();
        let c = context_for(s.conn(), "w-b1", Locale::Zh).unwrap();
        assert_eq!(c.label, "名字-w-b1");
        assert!(
            c.extension.as_deref().is_some_and(|e| e.contains('-')),
            "{:?}",
            c.extension
        );
        assert_eq!(
            c.root_human_channel_id,
            crewd_store::channels::human_id("crew-a"),
            "根是 crew-a"
        );
        assert!(!c.builtin_crew);
        assert_eq!(
            (c.quota_claude.clone(), c.quota_codex.clone()),
            (None, None),
            "quota 由 M3c 填"
        );
        assert!(!render(&c).is_empty());
    }

    #[test]
    fn states_the_ruled_behaviours() {
        // Controller rulings: profile change = next start (B8); ext prefix on speaker labels;
        // untargeted human post routing; approvals do not block; @human in an unwatched channel = do-not-disturb.
        let zh = render(&ctx(Locale::Zh, Role::Worker, false));
        let en = render(&ctx(Locale::En, Role::Worker, false));
        for (md, wants) in [
            (
                &zh,
                vec![
                    "下次启动时生效",
                    "（3号）名字",
                    "7-3",
                    "群主；没有群主就叫私聊里的另一方；再没有就叫机长",
                    "免打扰",
                    "你对该工具的下一次调用（不论参数）会放行一次",
                    "不论同意还是不同意",
                    "他正在注视、且你是成员的群",
                    "没有分机号的（例如管理员）",
                ],
            ),
            (
                &en,
                vec![
                    "at your next start",
                    "(ext 3) name",
                    "7-3",
                    "the first of these that is in the channel: the owner, otherwise the dm's other party, otherwise the captain",
                    "do-not-disturb",
                    "your next call to that tool (with any arguments) goes through once",
                    "whether the human allows or denies",
                    "a channel they are watching that you are a member of",
                    "without an extension (an admin, for instance)",
                    "the human decides",
                ],
            ),
        ] {
            for w in wants {
                assert!(md.contains(w), "missing {w}");
            }
        }
        for gone in [
            "本回合结束后生效",
            "after the current turn",
            "可以关联到工作项",
            "may link it",
            "-1 恒定",
            "-1 is always",
        ] {
            assert!(!zh.contains(gone) && !en.contains(gone), "{gone}");
        }
    }

    #[test]
    fn builtin_crew_gets_the_builtin_sections_and_not_the_normal_ones() {
        for l in [Locale::Zh, Locale::En] {
            let f = file(l);
            let chief_cap = render(&ctx(l, Role::Captain, true));
            let chief_wk = render(&ctx(l, Role::Worker, true));
            let cap = render(&ctx(l, Role::Captain, false));
            assert!(
                chief_cap.contains(f.get("ask.builtin"))
                    && !chief_cap.contains(f.get("ask.normal"))
            );
            assert!(
                chief_wk.contains(f.get("ask.builtin")) && !chief_wk.contains(f.get("ask.normal"))
            );
            assert!(
                chief_cap.contains(f.get("role.captain.builtin"))
                    && !chief_cap.contains(f.get("role.captain"))
            );
            assert!(cap.contains(f.get("ask.normal")) && !cap.contains(f.get("ask.builtin")));
            assert!(
                cap.contains(f.get("role.captain")) && !cap.contains(f.get("role.captain.builtin"))
            );
        }
    }

    #[test]
    fn captain_and_lineage_variants() {
        for l in [Locale::Zh, Locale::En] {
            let f = file(l);
            for role in [Role::Worker, Role::Captain] {
                let mut c = ctx(l, role, false);
                c.captain_name = None;
                c.parent_titles = vec![];
                c.child_titles = vec!["子甲".into(), "子乙".into()];
                let md = render(&c);
                assert!(
                    md.contains(f.get("captain.none")) == (role == Role::Worker),
                    "{l:?}/{role:?}"
                );
                assert!(
                    md.contains(f.get("lineage.root"))
                        && !md.contains(f.get("lineage.no_children"))
                );
                assert!(md.contains("子甲") && md.contains("子乙"), "{md}");
                let c = ctx(l, role, false);
                let md = render(&c);
                assert!(!md.contains(f.get("lineage.root")) && md.contains("上级示例"));
                assert!(md.contains(f.get("lineage.no_children")));
            }
        }
    }

    #[test]
    fn captain_owns_the_human_channel_only_in_a_root_crew() {
        let zh = file(Locale::Zh).get("role.captain");
        let en = file(Locale::En).get("role.captain");
        assert!(zh.contains("若本机组是根机组，人类群的群主也是你"), "{zh}");
        assert!(
            en.contains("if this is the root crew, you also own the human channel"),
            "{en}"
        );
        // The body bullet says the same.
        let body_zh = render(&ctx(Locale::Zh, Role::Worker, false));
        let body_en = render(&ctx(Locale::En, Role::Worker, false));
        assert!(body_zh.contains("根机组的人类群"), "{body_zh}");
        assert!(
            body_en.contains("the root crew's human channel"),
            "{body_en}"
        );
    }
}
