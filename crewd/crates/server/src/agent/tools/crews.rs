//! Captain tools, the crew itself (spec §22.3; T15 supplement, T15b): `rename_crew`,
//! `set_crew_status`, `delegation_policy`, `change_workdir`, and its place in the tree,
//! `crew_structure` (S69: create_child / create_parent / adopt / adopt_parent / release; removing,
//! restoring, purging and ordering crews are the human's).
//!
//! The target crew is the `crew` argument (number or id), else my own; acting on it needs
//! `acts_as_captain_of` (collab's rule, C2: the management captain counts as the captain of every
//! crew). `change_workdir` also reaches my crew's descendants. collab decides everything else
//! (an empty title, a removed crew, cycles, the management crew outside the tree …) and its
//! refusals reach the agent with their codes (C1). New and adopted crews go at the end of their
//! siblings (collab appends); nothing here reorders existing siblings (PM: no auto-sort).
use std::path::Path;

use crewd_proto::agent::CallResult;
use serde::Deserialize;
use serde_json::Value;

use super::captain::{not_your_crew, require_captain_of, target_crew, trimmed};
use super::channels::{Step, done};
use super::{Ctx, crew_arg, parse_args};
use crate::agent::Role;
use crate::agent::port::{self, CrewPatch, CrewView, NewCrewReq, PortError, StartReq};

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
    use crewd_collab::refusal::{Code, Refusal};
    let crew = target_crew(cx, arg).await?;
    // The management crew has no working directory of its own and gets none (I1): collab's code
    // for "the management crew stays as it is".
    if crew.management {
        return Err(cx.fail(PortError::Refused(port::Reason::Rule(Refusal::new(
            Code::CrewManagementFixed,
        )))));
    }
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

/// `crew_structure.action` (S69). Removing, restoring, purging and ordering crews are the human's:
/// they are not actions here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    CreateChild,
    CreateParent,
    Adopt,
    AdoptParent,
    Release,
}

impl Action {
    fn name(self) -> &'static str {
        match self {
            Self::CreateChild => "create_child",
            Self::CreateParent => "create_parent",
            Self::Adopt => "adopt",
            Self::AdoptParent => "adopt_parent",
            Self::Release => "release",
        }
    }

    /// (required, optional) arguments besides `action` and `crew`.
    fn shape(self) -> (&'static [&'static str], &'static [&'static str]) {
        match self {
            Self::CreateChild => (&["brief"], &["title", "runner"]),
            Self::CreateParent => (&[], &["title"]),
            Self::Adopt | Self::AdoptParent => (&["target"], &[]),
            Self::Release => (&["target"], &["to"]),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StructureArgs {
    action: Action,
    crew: Option<String>,
    target: Option<String>,
    to: Option<String>,
    title: Option<String>,
    brief: Option<String>,
    runner: Option<String>,
}

pub async fn crew_structure(cx: &Ctx<'_>, v: Value) -> CallResult {
    done(crew_structure_(cx, v).await)
}

/// What a new crew's captain is started with.
struct NewCaptain {
    brief: String,
    runner: Option<String>,
}

/// A crew's number as the receipts write it (`post(to: "N")`); every crew created here has one.
fn number(c: &CrewView) -> String {
    c.number.map(|n| n.to_string()).unwrap_or_default()
}

async fn crew_structure_(cx: &Ctx<'_>, v: Value) -> Step<CallResult> {
    let a: StructureArgs = parse_args(cx, v)?;
    let given = [
        ("target", &a.target),
        ("to", &a.to),
        ("title", &a.title),
        ("brief", &a.brief),
        ("runner", &a.runner),
    ];
    let (required, optional) = a.action.shape();
    let has = |n: &str| {
        given
            .iter()
            .any(|(k, v)| *k == n && v.as_deref().is_some_and(|s| !s.trim().is_empty()))
    };
    let stray = given
        .iter()
        .any(|(k, v)| v.is_some() && !required.contains(k) && !optional.contains(k));
    if stray || !required.iter().all(|r| has(r)) {
        let params = required
            .iter()
            .map(|r| r.to_string())
            .chain(optional.iter().map(|o| format!("{o}?")))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(cx.err(
            "r.structure_args",
            &[("action", a.action.name()), ("params", &params)],
        ));
    }
    let crew = crew_in_reach(cx, a.crew.as_deref()).await?;
    match a.action {
        Action::CreateChild => {
            let brief = trimmed(a.brief).unwrap_or_default();
            create_child(
                cx,
                &crew,
                trimmed(a.title),
                NewCaptain {
                    brief,
                    runner: trimmed(a.runner),
                },
            )
            .await
        }
        Action::CreateParent => create_parent(cx, &crew, trimmed(a.title)).await,
        Action::Adopt => {
            let target = crew_arg(cx, a.target.as_deref().unwrap_or_default()).await?;
            require_structure_reach(cx, &target).await?;
            adopt(cx, &target, &crew).await
        }
        Action::AdoptParent => {
            let target = crew_arg(cx, a.target.as_deref().unwrap_or_default()).await?;
            require_structure_reach(cx, &target).await?;
            adopt(cx, &crew, &target).await
        }
        Action::Release => {
            // `target` must be a direct child of `crew`: collab's `crew.not_direct_child`.
            let target = crew_arg(cx, a.target.as_deref().unwrap_or_default()).await?;
            let to = match trimmed(a.to) {
                Some(t) => {
                    let to = crew_arg(cx, &t).await?;
                    require_structure_reach(cx, &to).await?;
                    Some(to)
                }
                None => None,
            };
            release(cx, &crew, &target, to.as_ref()).await
        }
    }
}

/// Start the captain of a crew just created (it has none yet). The title is collab's rule.
async fn start_captain(
    cx: &Ctx<'_>,
    crew: &CrewView,
    c: NewCaptain,
) -> Result<(String, String), PortError> {
    let wanted = cx.r("r.default_captain_title", &[]);
    let title = port::resolve_title(Some(&wanted), Some(&c.brief))?;
    let id = port::start_session(
        cx.st,
        StartReq {
            crew_id: crew.id.clone(),
            role: Role::Captain,
            executor: c.runner,
            brief: c.brief,
            title: Some(title.clone()),
            model: None,
            effort: None,
            // A captain works in its crew's shared directory.
            isolation: false,
        },
    )
    .await?;
    Ok((cx.name(&title), id))
}

/// A child crew under `parent`, at the end of its children (collab appends; nothing is reordered,
/// S69). The brief goes into the new lobby first (System), so the human sees why it exists even
/// if its captain does not start (S81: then the crew stays and the result says not to create it
/// again).
async fn create_child(
    cx: &Ctx<'_>,
    parent: &CrewView,
    title: Option<String>,
    captain: NewCaptain,
) -> Step<CallResult> {
    let parent_name = cx.name(&parent.title);
    let title = title.unwrap_or_else(|| cx.r("r.default_child_title", &[("parent", &parent_name)]));
    let notice = cx.r(
        "r.child_brief_notice",
        &[("by", &cx.name(&cx.me.label)), ("brief", &captain.brief)],
    );
    let child = port::create_crew(
        cx.st,
        NewCrewReq {
            title,
            working_directory: parent.working_directory.clone().unwrap_or_default(),
            parent: Some(parent.id.clone()),
            child: None,
            notice: Some(notice),
        },
    )
    .await
    .map_err(|e| cx.fail(e))?;
    let name = cx.name(&child.title);
    let n = number(&child);
    match start_captain(cx, &child, captain).await {
        Ok((captain, id)) => Ok(cx.ok(
            "r.child_created",
            &[
                ("crew", &name),
                ("number", &n),
                ("parent", &parent_name),
                ("captain", &captain),
                ("id", &id),
            ],
        )),
        Err(e) => Err(cx.err(
            "r.child_created_no_captain",
            &[("crew", &name), ("number", &n), ("detail", &cx.detail(e))],
        )),
    }
}

/// `r.no_longer_top`: `crew` was top-level and is not any more (its human channel is archived and
/// its asks go to its new home's human channel, N1).
///
/// Only ever said after the change is made, so it cannot fail: if the new home cannot be read,
/// the line says so without naming it (an internal error is still logged by `Ctx::detail`).
async fn no_longer_top(cx: &Ctx<'_>, crew: &CrewView) -> String {
    let name = cx.name(&crew.title);
    let home = match port::home_of(cx.st, &crew.id).await {
        Ok(h) => port::crew(cx.st, &h)
            .await
            .map(|c| c.map(|c| cx.name(&c.title))),
        Err(e) => Err(e),
    };
    match home {
        Ok(Some(home)) => cx.r("r.no_longer_top", &[("crew", &name), ("home", &home)]),
        Ok(None) => cx.r("r.no_longer_top_unnamed", &[("crew", &name)]),
        Err(e) => {
            let _logged = cx.detail(e);
            cx.r("r.no_longer_top_unnamed", &[("crew", &name)])
        }
    }
}

fn was_top(c: &CrewView) -> bool {
    c.parent_ids.is_empty() && !c.management
}

/// A new top-level crew (at the end of the top level) with `child` under it, in one transaction.
async fn create_parent(cx: &Ctx<'_>, child: &CrewView, title: Option<String>) -> Step<CallResult> {
    let child_name = cx.name(&child.title);
    let title = title.unwrap_or_else(|| cx.r("r.default_parent_title", &[("child", &child_name)]));
    let notice = cx.r(
        "r.parent_notice",
        &[("by", &cx.name(&cx.me.label)), ("child", &child_name)],
    );
    let parent = port::create_crew(
        cx.st,
        NewCrewReq {
            title,
            working_directory: child.working_directory.clone().unwrap_or_default(),
            parent: None,
            child: Some(child.id.clone()),
            notice: Some(notice),
        },
    )
    .await
    .map_err(|e| cx.fail(e))?;
    // From here on the crew exists: nothing below may answer a plain failure (M1).
    let name = cx.name(&parent.title);
    let n = number(&parent);
    let brief = match &child.captain {
        Some(captain) => {
            let address = child
                .number
                .map(|n| n.to_string())
                .unwrap_or_else(|| captain.clone());
            cx.r(
                "r.parent_brief",
                &[
                    ("child", &child_name),
                    (
                        "child_arg",
                        &serde_json::to_string(&address).expect("string serialises"),
                    ),
                    (
                        "mentions_arg",
                        &serde_json::json!([{"kind":"session", "target_id":captain}]).to_string(),
                    ),
                ],
            )
        }
        None => cx.r("r.parent_brief_no_captain", &[("child", &child_name)]),
    };
    let started = start_captain(
        cx,
        &parent,
        NewCaptain {
            brief,
            runner: None,
        },
    )
    .await;
    let mut out = match &started {
        Ok((captain, id)) => cx.r(
            "r.parent_created",
            &[
                ("crew", &name),
                ("number", &n),
                ("child", &child_name),
                ("captain", captain),
                ("id", id),
            ],
        ),
        Err(e) => cx.r(
            "r.parent_created_no_captain",
            &[
                ("crew", &name),
                ("number", &n),
                ("child", &child_name),
                ("detail", &cx.detail(e.clone())),
            ],
        ),
    };
    if was_top(child) {
        out.push('\n');
        out.push_str(&no_longer_top(cx, child).await);
    }
    Ok(CallResult {
        text: out,
        is_error: started.is_err(),
    })
}

/// `child` goes under `parent` (at the end of its children). collab refuses cycles, the
/// management crew and removed crews.
async fn adopt(cx: &Ctx<'_>, child: &CrewView, parent: &CrewView) -> Step<CallResult> {
    let (child_name, parent_name) = (cx.name(&child.title), cx.name(&parent.title));
    let already = child.parent_ids.contains(&parent.id);
    port::attach_parent(cx.st, &child.id, &parent.id)
        .await
        .map_err(|e| cx.fail(e))?;
    if already {
        return Ok(cx.ok(
            "r.already_child",
            &[("child", &child_name), ("parent", &parent_name)],
        ));
    }
    let mut out = cx.r(
        "r.adopted",
        &[("child", &child_name), ("parent", &parent_name)],
    );
    if was_top(child) {
        out.push('\n');
        out.push_str(&no_longer_top(cx, child).await);
    }
    Ok(CallResult::ok(out))
}

/// Structure reach over `x` (controller ruling R0, T15b fix 1): I act as its captain (collab,
/// C2), or it is a direct child of a crew I act as the captain of. Every other crew of the
/// tree stays where it is unless the management captain or the human moves it (an org-level
/// change). Kept in the tool layer: collab's `adopt` / `release` also serve the human's RPC,
/// which has no session to check.
async fn require_structure_reach(cx: &Ctx<'_>, x: &CrewView) -> Step<()> {
    for id in std::iter::once(&x.id).chain(&x.parent_ids) {
        if port::acts_as_captain_of(cx.st, &cx.me.session_id, id)
            .await
            .map_err(|e| cx.fail(e))?
        {
            return Ok(());
        }
    }
    Err(not_your_crew(cx, x))
}

/// Detach `target` from `crew`, optionally under `to`. A crew whose last parent is gone becomes
/// top-level, at the end. collab refuses one that is not a direct child.
async fn release(
    cx: &Ctx<'_>,
    crew: &CrewView,
    target: &CrewView,
    to: Option<&CrewView>,
) -> Step<CallResult> {
    use port::ReleaseEffect;
    let effect = port::detach_parent(cx.st, &crew.id, &target.id, to.map(|t| t.id.clone()))
        .await
        .map_err(|e| cx.fail(e))?;
    let (name, from) = (cx.name(&target.title), cx.name(&crew.title));
    let key = match effect {
        ReleaseEffect::Unchanged => "r.released_same_parent",
        ReleaseEffect::ExistingParent => "r.released_existing_parent",
        ReleaseEffect::NewParent => "r.released_to",
        ReleaseEffect::TopLevel => "r.released_top",
        ReleaseEffect::Detached => "r.released",
    };
    let to_name = to.map(|t| cx.name(&t.title)).unwrap_or_default();
    Ok(cx.ok(key, &[("child", &name), ("from", &from), ("to", &to_name)]))
}

#[cfg(test)]
mod tests {
    use crewd_collab::refusal::{Code, Refusal};
    use crewd_proto::SenderKind;
    use serde_json::json;

    use crate::agent::Role;
    use crate::agent::port;
    use crate::agent::testkit::Fixture;
    use crate::agent::text::{self, Locale};
    use crate::agent::tools::call;
    use crate::control::{ControlError, FakeCall, FakeMethod, StartRequest};

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
        let mgmt = f.management_crew().await;
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
        let before = crew(&f, &other).await.working_directory.unwrap();
        let r = run(
            &f,
            "s-boss",
            "change_workdir",
            json!({"new_path": path, "crew": on, "confirm": true}),
        )
        .await;
        let one = text::r(
            Locale::Zh,
            "r.workdir_line",
            &[("crew", "示例他组"), ("old", &before)],
        );
        says(
            &r,
            false,
            "r.workdir_done",
            &[("path", &path), ("count", "1"), ("crews", &one)],
        );
        assert_eq!(
            crew(&f, &other).await.working_directory.as_deref(),
            Some(path.as_str())
        );
        // The management crew never: its captain, with no crew (its own) or by number "0" (I1).
        let mgmt = f.management_crew().await;
        for args in [
            json!({"new_path": path, "confirm": true}),
            json!({"new_path": path, "crew": "0", "confirm": true}),
            json!({"new_path": path, "crew": "0"}),
        ] {
            let r = run(&f, "s-boss", "change_workdir", args).await;
            refused(&r, Refusal::new(Code::CrewManagementFixed));
        }
        assert_eq!(crew(&f, &mgmt).await.working_directory, None);
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

    fn calls_of(f: &Fixture, prefix: &str) -> Vec<String> {
        f.control_calls()
            .into_iter()
            .filter(|c| c.starts_with(prefix))
            .collect()
    }

    /// The fake's next session id (`fake-<number of calls so far>`).
    fn next_fake_id(f: &Fixture) -> String {
        format!("fake-{}", f.control_calls().len())
    }

    /// The Start call of a new crew's captain, as the fake records it.
    fn captain_start(crew: &str, brief: &str, executor: &str) -> String {
        format!(
            "{:?}",
            FakeCall::Start(StartRequest {
                crew_id: crew.into(),
                role: Role::Captain,
                executor: executor.into(),
                brief: brief.into(),
                title: Some("机长".into()),
                model: None,
                effort: None,
                isolation: false,
            })
        )
    }

    async fn last_lobby_message(f: &Fixture, crew: &str) -> crewd_proto::Message {
        let lobby = port::lobby_of(&f.state, crew).await.unwrap().id;
        port::messages_raw(&f.state, &lobby, None, 1)
            .await
            .unwrap()
            .pop()
            .unwrap()
    }

    async fn top_level(f: &Fixture) -> Vec<String> {
        f.state
            .db
            .call(|s| crewd_collab::crews::top_level(s.conn()))
            .await
            .unwrap()
            .unwrap()
    }

    async fn crew_count(f: &Fixture) -> usize {
        f.state
            .db
            .call(|s| crewd_collab::crews::list(s.conn(), true))
            .await
            .unwrap()
            .unwrap()
            .len()
    }

    fn z(key: &str, vars: &[(&str, &str)]) -> String {
        text::r(Locale::Zh, key, vars)
    }

    #[tokio::test]
    async fn crew_structure_create_child_appends_and_starts_captain() {
        let f = Fixture::new().await;
        let a = f.add_crew("示例甲", Some(&f.crew)).await;
        let b = f.add_crew("示例乙", Some(&f.crew)).await;
        let id = next_fake_id(&f);
        let brief = "做语音重连：断线 3 秒内自动恢复";
        let r = run(
            &f,
            &f.captain,
            "crew_structure",
            json!({"action": "create_child", "brief": brief}),
        )
        .await;
        let kids = crew(&f, &f.crew).await.child_ids;
        assert_eq!(
            kids[..2],
            [a.clone(), b.clone()],
            "siblings keep their order"
        );
        assert_eq!(kids.len(), 3);
        let new = crew(&f, &kids[2]).await;
        let n = new.number.unwrap().to_string();
        says(
            &r,
            false,
            "r.child_created",
            &[
                ("crew", "示例机组·子机组"),
                ("number", &n),
                ("parent", "示例机组"),
                ("captain", "机长"),
                ("id", &id),
            ],
        );
        assert_eq!(new.title, "示例机组·子机组", "the md template");
        assert_eq!(
            new.working_directory,
            crew(&f, &f.crew).await.working_directory,
            "inherited"
        );
        assert_eq!(
            calls_of(&f, "Start("),
            [captain_start(&new.id, brief, "claude")]
        );
        let m = last_lobby_message(&f, &new.id).await;
        assert_eq!(
            m.text,
            z(
                "r.child_brief_notice",
                &[("by", "示例机长"), ("brief", brief)]
            )
        );
        assert_eq!(m.sender_kind, SenderKind::System);
        // With a title and a runner; the management captain under another crew.
        let mgmt = f.management_crew().await;
        f.add_session(&mgmt, "s-boss", Role::Captain, "示例总机长")
            .await;
        let an = number_of(&f, &a).await;
        let id = next_fake_id(&f);
        let r = run(
            &f,
            "s-boss",
            "crew_structure",
            json!({"action": "create_child", "crew": an, "brief": "示例任务", "title": "语音", "runner": "codex"}),
        )
        .await;
        let kid = crew(&f, &a).await.child_ids.pop().unwrap();
        let n = number_of(&f, &kid).await;
        says(
            &r,
            false,
            "r.child_created",
            &[
                ("crew", "语音"),
                ("number", &n),
                ("parent", "示例甲"),
                ("captain", "机长"),
                ("id", &id),
            ],
        );
        assert_eq!(
            calls_of(&f, "Start(").pop().unwrap(),
            captain_start(&kid, "示例任务", "codex")
        );
        // The top level is untouched by children.
        assert_eq!(top_level(&f).await, vec![f.crew.clone()]);
    }

    /// S81 / Q3: the crew stays; isError; the text says not to create it again.
    #[tokio::test]
    async fn create_child_without_captain_says_so() {
        let f = Fixture::new().await;
        f.fail_method(
            FakeMethod::Start,
            ControlError::Failed("示例：额度用完".into()),
        );
        let r = run(
            &f,
            &f.captain,
            "crew_structure",
            json!({"action": "create_child", "brief": "示例任务", "title": "语音"}),
        )
        .await;
        let kids = crew(&f, &f.crew).await.child_ids;
        assert_eq!(kids.len(), 1, "the crew is kept");
        let n = number_of(&f, &kids[0]).await;
        says(
            &r,
            true,
            "r.child_created_no_captain",
            &[
                ("crew", "语音"),
                ("number", &n),
                ("detail", "示例：额度用完"),
            ],
        );
        assert_eq!(calls_of(&f, "Start(").len(), 1);
        // The human still sees why it exists.
        let m = last_lobby_message(&f, &kids[0]).await;
        assert_eq!(
            m.text,
            z(
                "r.child_brief_notice",
                &[("by", "示例机长"), ("brief", "示例任务")]
            )
        );
    }

    #[tokio::test]
    async fn create_parent_warns_home_changes() {
        let f = Fixture::new().await;
        let other = f.add_crew("示例他组", None).await;
        let human = port::home_human_channel(&f.state, &f.crew).await.unwrap();
        assert!(!human.archived);
        let id = next_fake_id(&f);
        let r = run(
            &f,
            &f.captain,
            "crew_structure",
            json!({"action": "create_parent", "title": "示例总部"}),
        )
        .await;
        let parents = crew(&f, &f.crew).await.parent_ids;
        assert_eq!(parents.len(), 1);
        let parent = parents[0].clone();
        let n = number_of(&f, &parent).await;
        let expected = [
            z(
                "r.parent_created",
                &[
                    ("crew", "示例总部"),
                    ("number", &n),
                    ("child", "示例机组"),
                    ("captain", "机长"),
                    ("id", &id),
                ],
            ),
            z(
                "r.no_longer_top",
                &[("crew", "示例机组"), ("home", "示例总部")],
            ),
        ]
        .join("\n");
        assert_eq!(r.text, expected);
        assert!(!r.is_error);
        // The new crew is top-level at the end; mine left the top level; its human channel is
        // archived; the new captain was started with the md brief.
        assert_eq!(top_level(&f).await, [other.clone(), parent.clone()]);
        let human = port::channel(&f.state, &human.id).await.unwrap().unwrap();
        assert!(human.archived);
        let brief = z(
            "r.parent_brief",
            &[
                ("child", "示例机组"),
                (
                    "child_arg",
                    &serde_json::to_string(&number_of(&f, &f.crew).await).unwrap(),
                ),
                (
                    "mentions_arg",
                    &serde_json::json!([{"kind":"session", "target_id":f.captain}]).to_string(),
                ),
            ],
        );
        assert_eq!(
            calls_of(&f, "Start("),
            [captain_start(&parent, &brief, "claude")]
        );
        assert_eq!(
            last_lobby_message(&f, &parent).await.text,
            z(
                "r.parent_notice",
                &[("by", "示例机长"), ("child", "示例机组")]
            )
        );
        // A crew that already has a parent: no warning; a captain that does not start: isError,
        // both crews kept.
        f.fail_method(
            FakeMethod::Start,
            ControlError::Failed("示例：起不来".into()),
        );
        let r = run(
            &f,
            &f.captain,
            "crew_structure",
            json!({"action": "create_parent"}),
        )
        .await;
        let parents = crew(&f, &f.crew).await.parent_ids;
        assert_eq!(parents.len(), 2);
        let second = parents.into_iter().find(|p| *p != parent).unwrap();
        let n = number_of(&f, &second).await;
        says(
            &r,
            true,
            "r.parent_created_no_captain",
            &[
                ("crew", "示例机组·上级"),
                ("number", &n),
                ("child", "示例机组"),
                ("detail", "示例：起不来"),
            ],
        );
        assert_eq!(top_level(&f).await, [other, parent, second]);
    }

    /// I2 / M1: a parent whose captain does not start is kept; the result is an error that says
    /// who can start its captain (it is above an ordinary captain's reach) and still warns that
    /// my crew left the top level.
    #[tokio::test]
    async fn create_parent_without_captain_says_who_can_start_it() {
        let f = Fixture::new().await;
        f.fail_method(
            FakeMethod::Start,
            ControlError::Failed("示例：起不来".into()),
        );
        let r = run(
            &f,
            &f.captain,
            "crew_structure",
            json!({"action": "create_parent", "title": "示例总部"}),
        )
        .await;
        let parents = crew(&f, &f.crew).await.parent_ids;
        assert_eq!(parents.len(), 1, "the parent is kept");
        let n = number_of(&f, &parents[0]).await;
        assert_eq!(
            r.text,
            [
                z(
                    "r.parent_created_no_captain",
                    &[
                        ("crew", "示例总部"),
                        ("number", &n),
                        ("child", "示例机组"),
                        ("detail", "示例：起不来"),
                    ],
                ),
                z(
                    "r.no_longer_top",
                    &[("crew", "示例机组"), ("home", "示例总部")]
                ),
            ]
            .join("\n")
        );
        assert!(r.is_error);
        assert_eq!(top_level(&f).await, parents);
        // Its next step works: the management captain can hand it a captain.
        let mgmt = f.management_crew().await;
        f.add_session(&mgmt, "s-boss", Role::Captain, "示例总机长")
            .await;
        let id = next_fake_id(&f);
        let r = run(
            &f,
            "s-boss",
            "handoff_captain",
            json!({"crew": n, "runner": "claude"}),
        )
        .await;
        says(
            &r,
            false,
            "r.handoff_done_vacant",
            &[("crew", "示例总部"), ("new", "机长"), ("id", &id)],
        );
        // ... and an ordinary captain cannot (so the text does not send it there).
        let r = run(
            &f,
            &f.captain,
            "handoff_captain",
            json!({"crew": n, "runner": "claude"}),
        )
        .await;
        not_yours(&f, &r, &parents[0]).await;
    }

    #[tokio::test]
    async fn adopt_and_release_keep_order() {
        let f = Fixture::new().await;
        let a = f.add_crew("示例甲", Some(&f.crew)).await;
        let b = f.add_crew("示例乙", Some(&f.crew)).await;
        let t1 = f.add_crew("示例平级", None).await;
        let t2 = f.add_crew("示例另一组", None).await;
        assert_eq!(
            top_level(&f).await,
            [f.crew.clone(), t1.clone(), t2.clone()]
        );
        let n1 = number_of(&f, &t1).await;
        let cn = number_of(&f, &f.crew).await;
        // Moving a crew from another branch is the management captain's (R0).
        let mgmt = f.management_crew().await;
        f.add_session(&mgmt, "s-boss", Role::Captain, "示例总机长")
            .await;
        // adopt: appended to my children; it was top-level, so the warning names its new home.
        let r = run(
            &f,
            "s-boss",
            "crew_structure",
            json!({"action": "adopt", "crew": cn, "target": n1}),
        )
        .await;
        assert_eq!(
            r.text,
            [
                z(
                    "r.adopted",
                    &[("child", "示例平级"), ("parent", "示例机组")]
                ),
                z(
                    "r.no_longer_top",
                    &[("crew", "示例平级"), ("home", "示例机组")]
                ),
            ]
            .join("\n")
        );
        assert!(!r.is_error);
        assert_eq!(
            crew(&f, &f.crew).await.child_ids,
            [a.clone(), b.clone(), t1.clone()]
        );
        assert_eq!(top_level(&f).await, [f.crew.clone(), t2.clone()]);
        let r = run(
            &f,
            &f.captain,
            "crew_structure",
            json!({"action": "adopt", "target": t1}),
        )
        .await;
        says(
            &r,
            false,
            "r.already_child",
            &[("child", "示例平级"), ("parent", "示例机组")],
        );
        // release without `to`, last parent: top-level, at the end.
        let an = number_of(&f, &a).await;
        let r = run(
            &f,
            &f.captain,
            "crew_structure",
            json!({"action": "release", "target": an}),
        )
        .await;
        says(
            &r,
            false,
            "r.released_top",
            &[("child", "示例甲"), ("from", "示例机组")],
        );
        assert_eq!(top_level(&f).await, [f.crew.clone(), t2.clone(), a.clone()]);
        assert_eq!(crew(&f, &f.crew).await.child_ids, [b.clone(), t1.clone()]);
        // release `to` another child of mine: at the end of its children.
        let bn = number_of(&f, &b).await;
        let r = run(
            &f,
            &f.captain,
            "crew_structure",
            json!({"action": "release", "target": n1, "to": bn}),
        )
        .await;
        says(
            &r,
            false,
            "r.released_to",
            &[
                ("child", "示例平级"),
                ("from", "示例机组"),
                ("to", "示例乙"),
            ],
        );
        assert_eq!(crew(&f, &f.crew).await.child_ids, vec![b.clone()]);
        assert_eq!(crew(&f, &b).await.child_ids, vec![t1.clone()]);
        // `to` a crew that is neither my child nor mine to act on: refused, nothing moved.
        let d = f.add_crew("示例双亲", Some(&f.crew)).await;
        let n2 = number_of(&f, &t2).await;
        let dn = number_of(&f, &d).await;
        let r = run(
            &f,
            &f.captain,
            "crew_structure",
            json!({"action": "release", "target": dn, "to": n2}),
        )
        .await;
        not_yours(&f, &r, &t2).await;
        assert_eq!(crew(&f, &f.crew).await.child_ids, [b.clone(), d.clone()]);
        // A crew with another parent stays under it.
        f.rpc("crew.adopt", json!({"crewId": d, "parentId": a}))
            .await;
        let r = run(
            &f,
            &f.captain,
            "crew_structure",
            json!({"action": "release", "target": dn}),
        )
        .await;
        says(
            &r,
            false,
            "r.released",
            &[("child", "示例双亲"), ("from", "示例机组")],
        );
        assert_eq!(crew(&f, &d).await.parent_ids, vec![a.clone()]);
        // adopt_parent: my crew goes under another top-level crew, at the end of its children.
        let r = run(
            &f,
            "s-boss",
            "crew_structure",
            json!({"action": "adopt_parent", "crew": cn, "target": n2}),
        )
        .await;
        assert_eq!(
            r.text,
            [
                z(
                    "r.adopted",
                    &[("child", "示例机组"), ("parent", "示例另一组")]
                ),
                z(
                    "r.no_longer_top",
                    &[("crew", "示例机组"), ("home", "示例另一组")]
                ),
            ]
            .join("\n")
        );
        assert_eq!(crew(&f, &t2).await.child_ids, vec![f.crew.clone()]);
        assert_eq!(top_level(&f).await, [t2, a]);
    }

    /// R0: adopt / adopt_parent / release(to:) reach only crews I act as the captain of and their
    /// direct children; the management captain reaches every crew.
    #[tokio::test]
    async fn structure_targets_need_structure_reach() {
        let f = Fixture::new().await;
        let a = f.add_crew("示例甲", Some(&f.crew)).await;
        let b = f.add_crew("示例乙", Some(&f.crew)).await;
        let stranger = f.add_crew("示例陌生", None).await;
        f.add_session(&stranger, "s-st", Role::Captain, "陌生机长")
            .await;
        let human = port::home_human_channel(&f.state, &stranger).await.unwrap();
        let sn = number_of(&f, &stranger).await;
        let an = number_of(&f, &a).await;
        let bn = number_of(&f, &b).await;
        let cn = number_of(&f, &f.crew).await;
        // An ordinary captain: a stranger crew is refused as a target, nothing changes.
        for args in [
            json!({"action": "adopt", "target": sn}),
            json!({"action": "adopt_parent", "target": sn}),
            json!({"action": "release", "target": an, "to": sn}),
        ] {
            let r = run(&f, &f.captain, "crew_structure", args).await;
            not_yours(&f, &r, &stranger).await;
        }
        assert!(crew(&f, &stranger).await.parent_ids.is_empty());
        assert!(crew(&f, &f.crew).await.parent_ids.is_empty());
        assert_eq!(
            crew(&f, &f.crew).await.child_ids,
            vec![a.clone(), b.clone()]
        );
        let human = port::channel(&f.state, &human.id).await.unwrap().unwrap();
        assert!(!human.archived, "the refused adopt archived nothing");
        // My other direct child is in reach.
        let r = run(
            &f,
            &f.captain,
            "crew_structure",
            json!({"action": "release", "target": an, "to": bn}),
        )
        .await;
        says(
            &r,
            false,
            "r.released_to",
            &[("child", "示例甲"), ("from", "示例机组"), ("to", "示例乙")],
        );
        assert_eq!(crew(&f, &b).await.child_ids, vec![a.clone()]);
        // A grandchild is not (its parent is another captain's crew).
        let r = run(
            &f,
            &f.captain,
            "crew_structure",
            json!({"action": "adopt", "target": an}),
        )
        .await;
        not_yours(&f, &r, &a).await;
        // The management captain does all of them.
        let mgmt = f.management_crew().await;
        f.add_session(&mgmt, "s-boss", Role::Captain, "示例总机长")
            .await;
        let r = run(
            &f,
            "s-boss",
            "crew_structure",
            json!({"action": "release", "crew": bn, "target": an, "to": sn}),
        )
        .await;
        says(
            &r,
            false,
            "r.released_to",
            &[("child", "示例甲"), ("from", "示例乙"), ("to", "示例陌生")],
        );
        let r = run(
            &f,
            "s-boss",
            "crew_structure",
            json!({"action": "adopt", "crew": cn, "target": sn}),
        )
        .await;
        assert_eq!(
            r.text,
            [
                z(
                    "r.adopted",
                    &[("child", "示例陌生"), ("parent", "示例机组")]
                ),
                z(
                    "r.no_longer_top",
                    &[("crew", "示例陌生"), ("home", "示例机组")]
                ),
            ]
            .join("\n")
        );
        let human = port::channel(&f.state, &human.id).await.unwrap().unwrap();
        assert!(human.archived);
        let other = f.add_crew("示例上级", None).await;
        let on = number_of(&f, &other).await;
        let r = run(
            &f,
            "s-boss",
            "crew_structure",
            json!({"action": "adopt_parent", "crew": bn, "target": on}),
        )
        .await;
        says(
            &r,
            false,
            "r.adopted",
            &[("child", "示例乙"), ("parent", "示例上级")],
        );
        assert_eq!(crew(&f, &b).await.parent_ids.len(), 2);
    }

    #[tokio::test]
    async fn structure_refusals_pass_through() {
        let f = Fixture::new().await;
        let child = f.add_crew("示例子机组", Some(&f.crew)).await;
        let stranger = f.add_crew("示例陌生", None).await;
        let call =
            |action: &'static str, target: String| json!({"action": action, "target": target});
        // The management crew: out of an ordinary captain's reach (R0); for the management captain
        // collab refuses it, being outside the tree (number "0").
        let r = run(&f, &f.captain, "crew_structure", call("adopt", "0".into())).await;
        not_yours(&f, &r, &f.management_crew().await).await;
        let mgmt = f.management_crew().await;
        f.add_session(&mgmt, "s-boss", Role::Captain, "示例总机长")
            .await;
        let cn = number_of(&f, &f.crew).await;
        let r = run(
            &f,
            "s-boss",
            "crew_structure",
            json!({"action": "adopt", "crew": cn, "target": "0"}),
        )
        .await;
        refused(&r, Refusal::new(Code::CrewManagementNotInTree));
        // A cycle: my child as my parent.
        let r = run(
            &f,
            &f.captain,
            "crew_structure",
            call("adopt_parent", child.clone()),
        )
        .await;
        refused(
            &r,
            Refusal::new(Code::CrewWouldCycle)
                .arg("parent", child.as_str())
                .arg("crew", f.crew.as_str()),
        );
        // Myself.
        let r = run(
            &f,
            &f.captain,
            "crew_structure",
            call("adopt", f.crew.clone()),
        )
        .await;
        refused(&r, Refusal::new(Code::CrewCannotParentSelf));
        // Not a direct child.
        let r = run(
            &f,
            &f.captain,
            "crew_structure",
            call("release", stranger.clone()),
        )
        .await;
        refused(
            &r,
            Refusal::new(Code::CrewNotDirectChild)
                .arg("crew", stranger.as_str())
                .arg("from", f.crew.as_str()),
        );
        // A removed crew, by number.
        let gone = f.add_crew("示例已移除", None).await;
        let gn = number_of(&f, &gone).await;
        f.rpc("crew.remove", json!({"crewId": gone})).await;
        let r = run(&f, &f.captain, "crew_structure", call("adopt", gn.clone())).await;
        refused(
            &r,
            Refusal::new(Code::DirectoryCrewRemoved).arg("number", gn.as_str()),
        );
        // Another crew as `crew`: not mine.
        let sn = number_of(&f, &stranger).await;
        let r = run(
            &f,
            &f.captain,
            "crew_structure",
            json!({"action": "create_child", "crew": sn, "brief": "x"}),
        )
        .await;
        not_yours(&f, &r, &stranger).await;
        // The management captain on its own crew: collab refuses, nothing is created (one
        // transaction), nothing is started.
        let before = crew_count(&f).await;
        for args in [
            json!({"action": "create_child", "brief": "x"}),
            json!({"action": "create_parent"}),
        ] {
            let r = run(&f, "s-boss", "crew_structure", args).await;
            refused(&r, Refusal::new(Code::CrewManagementNotInTree));
        }
        assert_eq!(crew_count(&f).await, before);
        assert!(calls_of(&f, "Start(").is_empty());
        // ... while it may act on any other crew.
        let r = run(
            &f,
            "s-boss",
            "crew_structure",
            json!({"action": "adopt", "crew": sn, "target": child}),
        )
        .await;
        says(
            &r,
            false,
            "r.adopted",
            &[("child", "示例子机组"), ("parent", "示例陌生")],
        );
        assert_eq!(crew(&f, &child).await.parent_ids.len(), 2);
    }

    /// S69 / PM: no remove, restore, purge or reorder; each action takes its own arguments only.
    #[tokio::test]
    async fn structure_has_no_remove_or_purge() {
        let f = Fixture::new().await;
        let schema = crate::agent::catalog::input_schema(
            crate::agent::catalog::find("crew_structure").unwrap(),
            Locale::Zh,
        );
        assert_eq!(
            schema["properties"]["action"]["enum"],
            json!([
                "create_child",
                "create_parent",
                "adopt",
                "adopt_parent",
                "release"
            ])
        );
        for name in crate::agent::catalog::STRUCTURE_ACTIONS {
            let a: super::Action = serde_json::from_value(json!(name)).unwrap();
            assert_eq!(a.name(), *name);
        }
        for action in ["remove", "restore", "purge", "reorder", "archive"] {
            let r = run(
                &f,
                &f.captain,
                "crew_structure",
                json!({"action": action, "target": f.crew}),
            )
            .await;
            let detail = format!(
                "unknown variant `{action}`, expected one of `create_child`, `create_parent`, `adopt`, `adopt_parent`, `release`"
            );
            says(&r, true, "r.bad_args", &[("detail", &detail)]);
        }
        let before = crew_count(&f).await;
        for (args, action, params) in [
            (
                json!({"action": "create_child"}),
                "create_child",
                "brief, title?, runner?",
            ),
            (
                json!({"action": "create_child", "brief": " "}),
                "create_child",
                "brief, title?, runner?",
            ),
            (
                json!({"action": "adopt", "target": "1", "brief": "x"}),
                "adopt",
                "target",
            ),
            (
                json!({"action": "create_parent", "target": "1"}),
                "create_parent",
                "title?",
            ),
            (json!({"action": "release"}), "release", "target, to?"),
        ] {
            let r = run(&f, &f.captain, "crew_structure", args).await;
            says(
                &r,
                true,
                "r.structure_args",
                &[("action", action), ("params", params)],
            );
        }
        assert_eq!(crew_count(&f).await, before);
        assert!(f.control_calls().is_empty());
    }
}
