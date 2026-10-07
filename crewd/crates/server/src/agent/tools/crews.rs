//! Captain tools, the crew itself (spec §22.3; T15 supplement, T15b): `rename_crew`,
//! `set_crew_status`, `delegation_policy`, `change_workdir`.
//!
//! The target crew is the `crew` argument (number or id), else my own; acting on it needs
//! `acts_as_captain_of` (collab's rule, C2: the management captain counts as the captain of every
//! crew). `change_workdir` also reaches my crew's descendants. collab decides everything else
//! (an empty title, a removed crew …) and its refusals reach the agent with their codes (C1).
use std::path::Path;

use crewd_proto::agent::CallResult;
use serde::Deserialize;
use serde_json::Value;

use super::captain::{not_your_crew, require_captain_of, target_crew};
use super::channels::{Step, done};
use super::{Ctx, parse_args};
use crate::agent::port::{self, CrewPatch, CrewView};

/// Longest crew status line, in characters (`set_crew_status.text`).
const STATUS_MAX_CHARS: usize = 200;

/// The crew a call acts on, within reach: `crew` (else mine), and I act as its captain.
async fn crew_in_reach(cx: &Ctx<'_>, arg: Option<&str>) -> Step<CrewView> {
    let crew = target_crew(cx, arg).await?;
    require_captain_of(cx, &crew).await?;
    Ok(crew)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameArgs {
    name: String,
    crew: Option<String>,
}

pub async fn rename_crew(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(rename_crew_(cx, v).await)
}

async fn rename_crew_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let a: RenameArgs = parse_args(cx, v)?;
    let crew = crew_in_reach(cx, a.crew.as_deref()).await?;
    // An empty name is collab's refusal (`crew.title_empty`), as is a removed crew.
    let patch = CrewPatch {
        title: Some(a.name.clone()),
        ..CrewPatch::default()
    };
    port::update_crew(cx.st, &crew.id, patch)
        .await
        .map_err(|e| cx.fail(e))?;
    Ok(cx.ok(
        "r.renamed",
        &[
            ("old", &cx.name(&crew.title)),
            ("name", &cx.name(a.name.trim())),
        ],
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusArgs {
    text: String,
    crew: Option<String>,
}

pub async fn set_crew_status(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(set_crew_status_(cx, v).await)
}

async fn set_crew_status_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let a: StatusArgs = parse_args(cx, v)?;
    let text = a.text.trim();
    let count = text.chars().count();
    if count > STATUS_MAX_CHARS {
        return Err(cx.err(
            "r.status_too_long",
            &[
                ("count", &count.to_string()),
                ("max", &STATUS_MAX_CHARS.to_string()),
            ],
        ));
    }
    let crew = crew_in_reach(cx, a.crew.as_deref()).await?;
    // "" clears the line (collab's `CrewPatch.status_line`).
    let patch = CrewPatch {
        status_line: Some(text.to_string()),
        ..CrewPatch::default()
    };
    port::update_crew(cx.st, &crew.id, patch)
        .await
        .map_err(|e| cx.fail(e))?;
    let name = cx.name(&crew.title);
    Ok(if text.is_empty() {
        cx.ok("r.status_cleared", &[("crew", &name)])
    } else {
        cx.ok("r.status_set", &[("crew", &name), ("text", text)])
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyArgs {
    text: Option<String>,
    crew: Option<String>,
}

pub async fn delegation_policy(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(delegation_policy_(cx, v).await)
}

/// No `text` = read; with `text` = replace the whole policy (S70: one tool for both).
async fn delegation_policy_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let a: PolicyArgs = parse_args(cx, v)?;
    let crew = crew_in_reach(cx, a.crew.as_deref()).await?;
    let name = cx.name(&crew.title);
    let Some(text) = a.text else {
        return Ok(match crew.policy.filter(|p| !p.trim().is_empty()) {
            Some(p) => cx.ok("r.policy", &[("crew", &name), ("text", &p)]),
            None => cx.ok("r.policy_empty", &[("crew", &name)]),
        });
    };
    let text = text.trim();
    if text.is_empty() {
        return Err(cx.err("r.policy_blank", &[]));
    }
    let patch = CrewPatch {
        policy: Some(text.to_string()),
        ..CrewPatch::default()
    };
    port::update_crew(cx.st, &crew.id, patch)
        .await
        .map_err(|e| cx.fail(e))?;
    Ok(cx.ok("r.policy_updated", &[("crew", &name)]))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkdirArgs {
    new_path: String,
    crew: Option<String>,
    include_children: Option<bool>,
    confirm: Option<bool>,
}

pub async fn change_workdir(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(change_workdir_(cx, v).await)
}

/// `root` and every crew below it (breadth first, each once in a DAG), removed ones left out
/// (they are frozen: collab refuses any write to them).
async fn subtree(cx: &Ctx<'_>, root: CrewView) -> Step<Vec<CrewView>> {
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        for id in out[i].child_ids.clone() {
            if out.iter().any(|c| c.id == id) {
                continue;
            }
            if let Some(c) = port::crew(cx.st, &id).await.map_err(|e| cx.fail(e))?
                && !c.removed
            {
                out.push(c);
            }
        }
        i += 1;
    }
    Ok(out)
}

/// Whose directory I may change: a crew I act as the captain of (collab, C2: mine; every crew
/// for the management captain), or any crew below mine.
async fn workdir_crew(cx: &Ctx<'_>, arg: Option<&str>) -> Step<CrewView> {
    let crew = target_crew(cx, arg).await?;
    if port::acts_as_captain_of(cx.st, &cx.me.session_id, &crew.id)
        .await
        .map_err(|e| cx.fail(e))?
    {
        return Ok(crew);
    }
    let mine = target_crew(cx, None).await?;
    if subtree(cx, mine).await?.iter().any(|c| c.id == crew.id) {
        return Ok(crew);
    }
    Err(not_your_crew(cx, &crew))
}

async fn change_workdir_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let a: WorkdirArgs = parse_args(cx, v)?;
    let path = a.new_path.trim();
    if !Path::new(path).is_absolute() || !Path::new(path).is_dir() {
        return Err(cx.err("r.workdir_missing", &[("path", path)]));
    }
    let crew = workdir_crew(cx, a.crew.as_deref()).await?;
    let affected = if a.include_children.unwrap_or(true) {
        subtree(cx, crew).await?
    } else {
        vec![crew]
    };
    let lines = affected
        .iter()
        .map(|c| {
            let old = c
                .working_directory
                .clone()
                .unwrap_or_else(|| cx.r("r.workdir_none", &[]));
            cx.r(
                "r.workdir_line",
                &[("crew", &cx.name(&c.title)), ("old", &old)],
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let vars = [
        ("path", path),
        ("count", &affected.len().to_string()),
        ("crews", &lines),
    ];
    if !a.confirm.unwrap_or(false) {
        return Ok(cx.ok("r.workdir_preview", &vars));
    }
    let ids = affected.iter().map(|c| c.id.clone()).collect();
    port::set_working_directory(cx.st, ids, path.to_string())
        .await
        .map_err(|e| cx.fail(e))?;
    Ok(cx.ok("r.workdir_done", &vars))
}

#[cfg(test)]
mod tests {
    use crewd_collab::refusal::{Code, Refusal};
    use serde_json::json;

    use crate::agent::Role;
    use crate::agent::port;
    use crate::agent::testkit::Fixture;
    use crate::agent::text::{self, Locale};
    use crate::agent::tools::call;

    async fn run(
        f: &Fixture,
        who: &str,
        tool: &str,
        args: serde_json::Value,
    ) -> crewd_proto::agent::CallResult {
        let me = f.me(who).await;
        call(&f.state, &me, tool, args).await
    }

    /// The call was refused by collab with exactly this rule (C1: the code, not a sentence).
    #[track_caller]
    fn refused(r: &crewd_proto::agent::CallResult, rule: Refusal) {
        assert!(r.is_error, "expected a refusal, got: {}", r.text);
        assert_eq!(
            r.text,
            text::r(
                Locale::Zh,
                "r.failed",
                &[("detail", &rule.text(Locale::Zh))]
            )
        );
    }

    /// The tool's own receipt, whole: `is_error` as given.
    #[track_caller]
    fn says(r: &crewd_proto::agent::CallResult, error: bool, key: &str, vars: &[(&str, &str)]) {
        assert_eq!(r.text, text::r(Locale::Zh, key, vars));
        assert_eq!(r.is_error, error, "{}", r.text);
    }

    async fn management_crew(f: &Fixture) -> String {
        f.state
            .db
            .call(|s| crewd_collab::crews::management(s.conn()))
            .await
            .unwrap()
            .unwrap()
            .expect("crew.create creates the management crew")
    }

    async fn crew(f: &Fixture, id: &str) -> port::CrewView {
        port::crew(&f.state, id).await.unwrap().unwrap()
    }

    async fn number_of(f: &Fixture, id: &str) -> String {
        crew(f, id).await.number.unwrap().to_string()
    }

    async fn status_line(f: &Fixture, id: &str) -> Option<String> {
        let id = id.to_string();
        f.state
            .db
            .call(move |s| crewd_collab::crews::get(s.conn(), &id))
            .await
            .unwrap()
            .unwrap()
            .status_line
    }

    /// `r.not_your_crew` for `id`, as the tool writes it.
    async fn not_yours(f: &Fixture, r: &crewd_proto::agent::CallResult, id: &str) {
        let c = crew(f, id).await;
        let n = number_of(f, id).await;
        says(
            r,
            true,
            "r.not_your_crew",
            &[("crew", &c.title), ("number", &n)],
        );
    }

    /// The management captain (`s-boss`) and another top-level crew with its own captain.
    async fn boss_and_other(f: &Fixture) -> String {
        let mgmt = management_crew(f).await;
        f.add_session(&mgmt, "s-boss", Role::Captain, "示例总机长")
            .await;
        let other = f.add_crew("示例他组", None).await;
        f.add_session(&other, "s-ocap", Role::Captain, "他组机长")
            .await;
        other
    }

    #[tokio::test]
    async fn crew_attributes_with_crew_param() {
        let f = Fixture::new().await;
        let other = boss_and_other(&f).await;
        let on = number_of(&f, &other).await;
        // rename: own crew; an empty name is collab's refusal.
        let r = run(&f, &f.captain, "rename_crew", json!({"name": " 鉴权重构 "})).await;
        says(
            &r,
            false,
            "r.renamed",
            &[("old", "示例机组"), ("name", "鉴权重构")],
        );
        assert_eq!(crew(&f, &f.crew).await.title, "鉴权重构");
        let r = run(&f, &f.captain, "rename_crew", json!({"name": "  "})).await;
        refused(&r, Refusal::new(Code::CrewTitleEmpty));
        // Another crew: not mine; the management captain may, by number.
        let r = run(
            &f,
            &f.captain,
            "rename_crew",
            json!({"name": "x", "crew": on}),
        )
        .await;
        not_yours(&f, &r, &other).await;
        let r = run(
            &f,
            "s-boss",
            "rename_crew",
            json!({"name": "语音", "crew": on}),
        )
        .await;
        says(
            &r,
            false,
            "r.renamed",
            &[("old", "示例他组"), ("name", "语音")],
        );
        // status: at most 200 characters (counted after trimming); "" clears it.
        let r = run(
            &f,
            &f.captain,
            "set_crew_status",
            json!({"text": "长".repeat(201)}),
        )
        .await;
        says(
            &r,
            true,
            "r.status_too_long",
            &[("count", "201"), ("max", "200")],
        );
        let line = format!(" {} ", "长".repeat(200));
        let r = run(&f, &f.captain, "set_crew_status", json!({"text": line})).await;
        says(
            &r,
            false,
            "r.status_set",
            &[("crew", "鉴权重构"), ("text", &"长".repeat(200))],
        );
        let r = run(
            &f,
            "s-boss",
            "set_crew_status",
            json!({"text": "在等 CI", "crew": on}),
        )
        .await;
        says(
            &r,
            false,
            "r.status_set",
            &[("crew", "语音"), ("text", "在等 CI")],
        );
        assert_eq!(status_line(&f, &other).await.as_deref(), Some("在等 CI"));
        let r = run(&f, &f.captain, "set_crew_status", json!({"text": ""})).await;
        says(&r, false, "r.status_cleared", &[("crew", "鉴权重构")]);
        assert_eq!(status_line(&f, &f.crew).await, None);
        let r = run(
            &f,
            &f.captain,
            "set_crew_status",
            json!({"text": "x", "crew": on}),
        )
        .await;
        not_yours(&f, &r, &other).await;
        // policy: read (none yet), replace, read back; blank text refused.
        let r = run(&f, &f.captain, "delegation_policy", json!({})).await;
        says(&r, false, "r.policy_empty", &[("crew", "鉴权重构")]);
        let r = run(
            &f,
            &f.captain,
            "delegation_policy",
            json!({"text": " 小活自己做，大活起 worker "}),
        )
        .await;
        says(&r, false, "r.policy_updated", &[("crew", "鉴权重构")]);
        let r = run(&f, &f.captain, "delegation_policy", json!({})).await;
        says(
            &r,
            false,
            "r.policy",
            &[("crew", "鉴权重构"), ("text", "小活自己做，大活起 worker")],
        );
        let r = run(&f, &f.captain, "delegation_policy", json!({"text": " "})).await;
        says(&r, true, "r.policy_blank", &[]);
        let r = run(&f, &f.captain, "delegation_policy", json!({"crew": on})).await;
        not_yours(&f, &r, &other).await;
        let r = run(
            &f,
            "s-boss",
            "delegation_policy",
            json!({"text": "只做语音", "crew": on}),
        )
        .await;
        says(&r, false, "r.policy_updated", &[("crew", "语音")]);
        let r = run(&f, "s-ocap", "delegation_policy", json!({})).await;
        says(
            &r,
            false,
            "r.policy",
            &[("crew", "语音"), ("text", "只做语音")],
        );
        // A removed crew: collab's refusal by number.
        f.rpc("crew.remove", json!({"crewId": other})).await;
        let r = run(
            &f,
            "s-boss",
            "rename_crew",
            json!({"name": "y", "crew": on}),
        )
        .await;
        refused(
            &r,
            Refusal::new(Code::DirectoryCrewRemoved).arg("number", on.as_str()),
        );
        // A worker has none of these.
        let r = run(&f, &f.worker, "delegation_policy", json!({})).await;
        says(&r, true, "r.captain_only", &[("name", "delegation_policy")]);
    }

    #[tokio::test]
    async fn change_workdir_previews_then_applies_within_reach() {
        let f = Fixture::new().await;
        let other = boss_and_other(&f).await;
        let child = f.add_crew("示例子机组", Some(&f.crew)).await;
        let grandchild = f.add_crew("示例孙机组", Some(&child)).await;
        let gone = f.add_crew("示例已移除", Some(&child)).await;
        f.rpc("crew.remove", json!({"crewId": gone})).await;
        let old = crew(&f, &f.crew).await.working_directory.unwrap();
        let new_dir = tempfile::tempdir().unwrap();
        let path = new_dir.path().display().to_string();
        let line = |name: &str| {
            text::r(
                Locale::Zh,
                "r.workdir_line",
                &[("crew", name), ("old", &old)],
            )
        };
        let three = [line("示例机组"), line("示例子机组"), line("示例孙机组")].join("\n");
        // Preview: nothing changes; the removed crew is left out.
        let r = run(&f, &f.captain, "change_workdir", json!({"new_path": path})).await;
        says(
            &r,
            false,
            "r.workdir_preview",
            &[("path", &path), ("count", "3"), ("crews", &three)],
        );
        assert_eq!(crew(&f, &f.crew).await.working_directory.unwrap(), old);
        // Apply.
        let r = run(
            &f,
            &f.captain,
            "change_workdir",
            json!({"new_path": path, "confirm": true}),
        )
        .await;
        says(
            &r,
            false,
            "r.workdir_done",
            &[("path", &path), ("count", "3"), ("crews", &three)],
        );
        for id in [&f.crew, &child, &grandchild] {
            assert_eq!(
                crew(&f, id).await.working_directory.as_deref(),
                Some(path.as_str())
            );
        }
        assert_eq!(crew(&f, &gone).await.working_directory.unwrap(), old);
        // A crew below mine, without its children.
        let other_dir = tempfile::tempdir().unwrap();
        let path2 = other_dir.path().display().to_string();
        let cn = number_of(&f, &child).await;
        let r = run(
            &f,
            &f.captain,
            "change_workdir",
            json!({"new_path": path2, "crew": cn, "include_children": false, "confirm": true}),
        )
        .await;
        let one = text::r(
            Locale::Zh,
            "r.workdir_line",
            &[("crew", "示例子机组"), ("old", &path)],
        );
        says(
            &r,
            false,
            "r.workdir_done",
            &[("path", &path2), ("count", "1"), ("crews", &one)],
        );
        assert_eq!(
            crew(&f, &grandchild).await.working_directory.as_deref(),
            Some(path.as_str())
        );
        // Not an existing absolute directory: refused before anything.
        for bad in ["/no/such/dir-示例", "relative/dir"] {
            let r = run(
                &f,
                &f.captain,
                "change_workdir",
                json!({"new_path": bad, "confirm": true}),
            )
            .await;
            says(&r, true, "r.workdir_missing", &[("path", bad)]);
        }
        // Another top-level crew: not mine; the management captain may.
        let on = number_of(&f, &other).await;
        let r = run(
            &f,
            &f.captain,
            "change_workdir",
            json!({"new_path": path, "crew": on, "confirm": true}),
        )
        .await;
        not_yours(&f, &r, &other).await;
        let r = run(
            &f,
            "s-boss",
            "change_workdir",
            json!({"new_path": path, "crew": on, "confirm": true}),
        )
        .await;
        assert!(!r.is_error, "{}", r.text);
        assert_eq!(
            crew(&f, &other).await.working_directory.as_deref(),
            Some(path.as_str())
        );
        // My parent is not below me.
        let parent = f.add_crew("示例上级", None).await;
        f.rpc("crew.adopt", json!({"crewId": f.crew, "parentId": parent}))
            .await;
        let pn = number_of(&f, &parent).await;
        let r = run(
            &f,
            &f.captain,
            "change_workdir",
            json!({"new_path": path, "crew": pn}),
        )
        .await;
        not_yours(&f, &r, &parent).await;
    }
}
