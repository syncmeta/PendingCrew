//! 把旧 Swift 版的 JSON 数据只读地导入 crewd 的 SQLite。源目录一个字节都不改。
mod crews;
mod model;
pub mod paths;
mod report;
mod rows;
mod sessions;

mod archive;
mod todos;
mod whiteboard;

use anyhow::Context as _;
pub use report::{ImportReport, Skipped};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub(crate) struct Ctx<'a> {
    pub src: &'a Path,
    pub report: &'a mut ImportReport,
    pub consumed: &'a mut BTreeSet<PathBuf>,
    pub rewrite: &'a paths::PathRewrite,
}

/// 只读导入整个旧数据目录到 `store`（应为空库）。全部写入在一个事务里，失败则什么都不留。
/// 附件在开事务之前复制到 `dest_data_dir/attachments`：复制失败则不提交任何东西
/// （残留的已复制文件无害，重试会覆盖）。
pub fn import(
    src: &Path,
    store: &mut crewd_store::Store,
    dest_data_dir: &Path,
) -> anyhow::Result<ImportReport> {
    anyhow::ensure!(src.is_dir(), "旧数据目录不存在: {}", src.display());
    // 必须在任何写入（附件复制）之前：数据目录落在旧目录里会把附件复制到自己身上。
    paths::check_disjoint(src, dest_data_dir)?;
    anyhow::ensure!(store.is_empty()?, "目标库不是空的，导入只写新库");
    let mut report = ImportReport::default();
    let att_from = src.join("attachments");
    let att_to = dest_data_dir.join("attachments");
    let copied = paths::copy_dir(&att_from, &att_to).with_context(|| {
        format!(
            "复制附件失败: {} → {}",
            att_from.display(),
            att_to.display()
        )
    })?;
    report.add("attachmentFiles", copied);
    let mut consumed = BTreeSet::new();
    let rewrite = paths::PathRewrite {
        from: att_from,
        to: att_to,
    };
    store.with_tx(|tx| {
        let mut cx = Ctx {
            src,
            report: &mut report,
            consumed: &mut consumed,
            rewrite: &rewrite,
        };
        let crews = crews::import(tx, &mut cx)?;
        sessions::import(tx, &mut cx, &crews)?;
        for c in &crews {
            whiteboard::import_board(tx, &mut cx, &c.id)?;
            todos::import(tx, &mut cx, &c.id, crewd_proto::Ledger::Agent)?;
            todos::import(tx, &mut cx, &c.id, crewd_proto::Ledger::Human)?;
        }
        whiteboard::import_outbox(tx, &mut cx)?;
        archive::import_rest(tx, &mut cx)?;
        // 大厅、游标钉末尾、机长指针、分机号、nextCrewNumber（local-crews.json 此时已进归档）。
        crewd_store::schema::backfill_v3(tx)?;
        // 总机组单根：最早的内建机组（或新建的 pendingcrew-chief）当组织根，其余顶层机组挂到它下面。
        crewd_store::schema::backfill_v4(tx)?;
        crewd_store::meta_set(tx, "importedAt", &crewd_store::now_ts())?;
        crewd_store::meta_set(tx, "importSource", &src.display().to_string())?;
        // 到这里所有计数都已完成：报告与数据同一个事务落库，不会出现有数据没报告。
        crewd_store::meta_set(tx, "importReport", &serde_json::to_string(cx.report)?)?;
        Ok(())
    })?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn ctx_parts(dir: &Path) -> (ImportReport, BTreeSet<PathBuf>, paths::PathRewrite) {
        (
            ImportReport::default(),
            BTreeSet::new(),
            paths::PathRewrite {
                from: dir.join("attachments"),
                to: PathBuf::from("/new/attachments"),
            },
        )
    }

    #[test]
    fn norm_ts_converts_offsets_and_keeps_unparseable() {
        let dir = tempfile::tempdir().unwrap();
        let (mut r, mut c, rw) = ctx_parts(dir.path());
        let mut cx = Ctx {
            src: dir.path(),
            report: &mut r,
            consumed: &mut c,
            rewrite: &rw,
        };
        assert_eq!(
            rows::norm_ts(&mut cx, "2026-09-11T12:34:56Z"),
            "2026-09-11T12:34:56.000Z"
        );
        assert_eq!(
            rows::norm_ts(&mut cx, "2026-09-11T12:34:56.789+08:00"),
            "2026-09-11T04:34:56.789Z"
        );
        assert_eq!(rows::norm_ts(&mut cx, "昨天"), "昨天");
        assert_eq!(r.counts.get("timestampsUnparsed"), Some(&1));
    }

    #[test]
    fn read_rows_skips_bad_rows_and_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.json");
        fs::write(
            &good,
            r#"[{"id":"a","mime":"x","path":"/p"},{"id":"b"},{"id":"c","mime":"y","path":"/q"}]"#,
        )
        .unwrap();
        let bad = dir.path().join("bad.json");
        fs::write(&bad, "{not json").unwrap();
        let obj = dir.path().join("obj.json");
        fs::write(&obj, r#"{"a":1}"#).unwrap();
        let (mut r, mut c, rw) = ctx_parts(dir.path());
        let mut cx = Ctx {
            src: dir.path(),
            report: &mut r,
            consumed: &mut c,
            rewrite: &rw,
        };
        let rows: Vec<model::LAttachment> = rows::read_rows(&mut cx, &good).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(
            rows::read_rows::<model::LAttachment>(&mut cx, &bad)
                .unwrap()
                .is_empty()
        );
        assert!(
            rows::read_rows::<model::LAttachment>(&mut cx, &obj)
                .unwrap()
                .is_empty()
        );
        assert!(
            rows::read_rows::<model::LAttachment>(&mut cx, &dir.path().join("missing.json"))
                .unwrap()
                .is_empty()
        );
        assert_eq!(r.skipped.len(), 3);
        assert_eq!(r.skipped[0].file, "good.json");
        assert_eq!(r.skipped[0].index, Some(1));
        assert!(
            !c.contains(&good),
            "有一行没解出来：整份文件留给归档原样保存，好行照常导入"
        );
        assert!(!c.contains(&bad), "坏文件不算 consumed，留给归档原样保存");
        let clean = dir.path().join("clean.json");
        fs::write(&clean, r#"[{"id":"a","mime":"x","path":"/p"}]"#).unwrap();
        let mut cx = Ctx {
            src: dir.path(),
            report: &mut r,
            consumed: &mut c,
            rewrite: &rw,
        };
        assert_eq!(
            rows::read_rows::<model::LAttachment>(&mut cx, &clean)
                .unwrap()
                .len(),
            1
        );
        assert!(c.contains(&clean), "每行都进库了才算 consumed");
    }

    #[test]
    fn path_rewrite_only_touches_paths_under_source() {
        let rw = paths::PathRewrite {
            from: "/old/attachments".into(),
            to: "/new/attachments".into(),
        };
        assert_eq!(
            rw.apply("/old/attachments/c1/a.png"),
            "/new/attachments/c1/a.png"
        );
        assert_eq!(rw.apply("/elsewhere/a.png"), "/elsewhere/a.png");
    }

    #[test]
    fn copy_dir_copies_recursively() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("a");
        fs::create_dir_all(from.join("c1")).unwrap();
        fs::write(from.join("c1/x.txt"), "x").unwrap();
        fs::write(from.join("y.txt"), "y").unwrap();
        let to = dir.path().join("b");
        assert_eq!(paths::copy_dir(&from, &to).unwrap(), 2);
        assert_eq!(fs::read_to_string(to.join("c1/x.txt")).unwrap(), "x");
        assert_eq!(paths::copy_dir(&dir.path().join("nope"), &to).unwrap(), 0);
    }

    #[test]
    fn copy_dir_refuses_to_copy_onto_itself() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("a");
        fs::create_dir_all(&from).unwrap();
        fs::write(from.join("y.txt"), "y").unwrap();
        assert!(paths::copy_dir(&from, &from).is_err());
        assert!(paths::copy_dir(&from, &from.join(".")).is_err());
        assert!(paths::copy_dir(&from, &from.join("sub")).is_err());
        assert_eq!(fs::read_to_string(from.join("y.txt")).unwrap(), "y");
        assert!(!from.join("sub").exists());
    }

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/basic")
    }

    #[test]
    fn crews_and_sessions_import_from_fixture() {
        let src = fixture();
        let store = crewd_store::Store::open_in_memory().unwrap();
        let (mut r, mut c, rw) = ctx_parts(&src);
        let mut cx = Ctx {
            src: &src,
            report: &mut r,
            consumed: &mut c,
            rewrite: &rw,
        };
        let crews = crews::import(store.conn(), &mut cx).unwrap();
        sessions::import(store.conn(), &mut cx, &crews).unwrap();

        let list = crewd_store::crews::list(store.conn()).unwrap();
        let ids: Vec<_> = list.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["local-aaaa", "local-bbbb", "pendingcrew-chief"]);
        let b = list.iter().find(|c| c.id == "local-bbbb").unwrap();
        assert_eq!(b.parent_ids, ["local-aaaa"]);
        let chief = list.iter().find(|c| c.id == "pendingcrew-chief").unwrap();
        assert!(chief.management);
        assert_eq!(
            list[0].captain_executor.as_deref(),
            Some("claude"),
            "captainAgentKind claude_code 导入为 captain_executor claude"
        );
        assert_eq!(list[0].created_at, "2026-09-01T08:00:00.000Z");

        let ses = crewd_store::sessions::list_for_crew(store.conn(), "local-aaaa").unwrap();
        assert_eq!(ses.len(), 3);
        let cap = ses.iter().find(|s| s.id == "captain-1111aaaa").unwrap();
        assert_eq!(cap.role, crewd_proto::Role::Captain);
        assert_eq!(cap.display_name.as_deref(), Some("机长"));
        let w = ses.iter().find(|s| s.id == "worker-2222bbbb").unwrap();
        assert_eq!(
            w.agent_session_id.as_deref(),
            Some("thread-xyz"),
            "取 updatedAt 最新的那条"
        );
        assert_eq!(w.model.as_deref(), Some("gpt-5.6"));
        let old = ses.iter().find(|s| s.id == "worker-3333cccc").unwrap();
        assert_eq!(old.executor, "unknown");
        assert_eq!(
            cap.executor, "claude",
            "claude_code 改名为 claude（spec §16）"
        );
        assert_eq!(w.executor, "codex");

        let extra_of = |id: &str| -> serde_json::Value {
            let s: Option<String> = store
                .conn()
                .query_row("SELECT extra FROM sessions WHERE id=?1", [id], |x| x.get(0))
                .unwrap();
            s.map(|s| serde_json::from_str(&s).unwrap())
                .unwrap_or(serde_json::Value::Null)
        };
        let w_extra = extra_of("worker-2222bbbb");
        assert_eq!(
            w_extra["member"]["extensionNumber"], 2,
            "成员表里的未知字段并进 session.extra.member：{w_extra}"
        );
        assert!(
            extra_of("worker-2222bbbb")
                .to_string()
                .contains(r#""extensionNumber":2"#)
        );
        let cap_extra = extra_of("captain-1111aaaa");
        assert_eq!(cap_extra["structuredPost"], true, "记录自己的 extra 保留");
        assert!(cap_extra.get("member").is_none(), "成员没有多余字段就不加");
        assert!(
            !c.contains(&src.join("local-crews.json")),
            "local-crews.json 总是原样归档（nextCrewNumber、成员 createdAt 等都在里面）"
        );

        assert_eq!(r.counts["crews"], 3);
        assert_eq!(r.counts["sessions"], 3);
        assert_eq!(r.counts["sessionsWithoutRecord"], 1);
        assert_eq!(r.counts["danglingParentEdges"], 1);
        assert_eq!(r.counts["sessionRecordsSuperseded"], 1);
        assert!(
            r.skipped
                .iter()
                .any(|s| s.file == "local-crews.json" && s.index == Some(3))
        );
    }

    fn legacy_root(crews_json: &str, sessions_json: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("local-crews.json"), crews_json).unwrap();
        fs::create_dir_all(dir.path().join("whiteboards")).unwrap();
        fs::write(
            dir.path().join("whiteboards/agent-sessions.json"),
            sessions_json,
        )
        .unwrap();
        dir
    }

    fn session_rec(session: &str, crew: &str, agent_sid: &str, updated: &str) -> String {
        format!(
            r#"{{"crewId":"{crew}","sessionId":"{session}","kind":"codex","agentSessionId":"{agent_sid}","updatedAt":"{updated}"}}"#
        )
    }

    #[test]
    fn newest_session_record_wins_by_instant_not_string() {
        let crews = r#"{"crews":[{"id":"c1","title":"t","createdAt":"2026-09-01T00:00:00Z","updatedAt":"2026-09-01T00:00:00Z"}]}"#;
        let recs = [
            // 原串比较会判 A 更新（'Z' > '.'）；按时刻 B 晚 500ms。
            session_rec("worker-a", "c1", "old", "2026-09-01T09:00:00Z"),
            session_rec("worker-a", "c1", "new", "2026-09-01T09:00:00.500Z"),
            // +08:00 的 10:00 是 02:00Z，比 03:00Z 早；原串比较会反过来。
            session_rec("worker-b", "c1", "old", "2026-09-01T10:00:00+08:00"),
            session_rec("worker-b", "c1", "new", "2026-09-01T03:00:00Z"),
            // 同一对，顺序反过来：后到的是更旧的那条，也不能覆盖。
            session_rec("worker-c", "c1", "new", "2026-09-01T03:00:00Z"),
            session_rec("worker-c", "c1", "old", "2026-09-01T10:00:00+08:00"),
        ];
        let src = legacy_root(crews, &format!("[{}]", recs.join(",")));
        let store = crewd_store::Store::open_in_memory().unwrap();
        let (mut r, mut c, rw) = ctx_parts(src.path());
        let mut cx = Ctx {
            src: src.path(),
            report: &mut r,
            consumed: &mut c,
            rewrite: &rw,
        };
        let imported = crews::import(store.conn(), &mut cx).unwrap();
        sessions::import(store.conn(), &mut cx, &imported).unwrap();

        let ses = crewd_store::sessions::list_for_crew(store.conn(), "c1").unwrap();
        assert_eq!(ses.len(), 3);
        for s in &ses {
            assert_eq!(s.agent_session_id.as_deref(), Some("new"), "{}", s.id);
        }
        assert_eq!(r.counts["sessionRecordsSuperseded"], 3);
    }

    #[test]
    fn session_for_unknown_crew_is_skipped_with_reason() {
        let crews = r#"{"crews":[{"id":"c1","title":"t","createdAt":"2026-09-01T00:00:00Z","updatedAt":"2026-09-01T00:00:00Z"}]}"#;
        let recs = session_rec("worker-x", "nope", "a", "2026-09-01T09:00:00Z");
        let src = legacy_root(crews, &format!("[{recs}]"));
        let store = crewd_store::Store::open_in_memory().unwrap();
        let (mut r, mut c, rw) = ctx_parts(src.path());
        let mut cx = Ctx {
            src: src.path(),
            report: &mut r,
            consumed: &mut c,
            rewrite: &rw,
        };
        let imported = crews::import(store.conn(), &mut cx).unwrap();
        sessions::import(store.conn(), &mut cx, &imported).unwrap();
        assert_eq!(r.counts["sessionsForUnknownCrew"], 1);
        assert!(!r.counts.contains_key("sessions"));
        let sk = r
            .skipped
            .iter()
            .find(|s| s.file == "whiteboards/agent-sessions.json")
            .unwrap();
        assert_eq!(sk.index, None);
        assert!(sk.reason.contains("worker-x") && sk.reason.contains("nope"));
    }

    #[test]
    fn crews_file_without_crews_array_is_skipped_and_not_consumed() {
        for body in [
            r#"{"version":1}"#,
            r#"{"crews":{"a":1}}"#,
            r#"[{"id":"x"}]"#,
        ] {
            let src = legacy_root(body, "[]");
            let store = crewd_store::Store::open_in_memory().unwrap();
            let (mut r, mut c, rw) = ctx_parts(src.path());
            let mut cx = Ctx {
                src: src.path(),
                report: &mut r,
                consumed: &mut c,
                rewrite: &rw,
            };
            let imported = crews::import(store.conn(), &mut cx).unwrap();
            assert!(imported.is_empty(), "{body}");
            assert_eq!(r.skipped.len(), 1, "{body}");
            assert_eq!(r.skipped[0].file, "local-crews.json");
            assert_eq!(r.skipped[0].index, None);
            assert!(r.skipped[0].reason.contains("crews"));
            assert!(!c.contains(&src.path().join("local-crews.json")), "{body}");
            assert!(
                crewd_store::crews::list(store.conn()).unwrap().is_empty(),
                "{body}"
            );
        }
    }

    #[test]
    fn duplicate_parent_ids_are_deduped() {
        let crews = r#"{"crews":[
            {"id":"p","title":"p","createdAt":"2026-09-01T00:00:00Z","updatedAt":"2026-09-01T00:00:00Z"},
            {"id":"k","title":"k","createdAt":"2026-09-01T00:00:00Z","updatedAt":"2026-09-01T00:00:00Z",
             "parentCrewIds":["p","p","gone","gone"]}]}"#;
        let src = legacy_root(crews, "[]");
        let store = crewd_store::Store::open_in_memory().unwrap();
        let (mut r, mut c, rw) = ctx_parts(src.path());
        let mut cx = Ctx {
            src: src.path(),
            report: &mut r,
            consumed: &mut c,
            rewrite: &rw,
        };
        crews::import(store.conn(), &mut cx).unwrap();
        let list = crewd_store::crews::list(store.conn()).unwrap();
        let k = list.iter().find(|c| c.id == "k").unwrap();
        assert_eq!(k.parent_ids, ["p"]);
        assert_eq!(r.counts["danglingParentEdges"], 1);
    }

    #[test]
    fn legacy_message_keeps_unknown_fields_in_extra() {
        let m: model::LMessage = serde_json::from_str(
            r#"{"id":"m","senderKind":"user","text":"t","createdAt":"2026-09-11T00:00:00Z",
                "mentions":[{"kind":"session","target_id":"worker-1"}],"audience":"human","headline":"h"}"#,
        )
        .unwrap();
        assert_eq!(m.mentions[0].target_id.as_deref(), Some("worker-1"));
        assert_eq!(m.extra["audience"], "human");
        assert!(!m.extra.contains_key("text"));
    }

    fn import_parts(src: &Path, store: &crewd_store::Store) -> ImportReport {
        let (mut r, mut c, rw) = ctx_parts(src);
        let mut cx = Ctx {
            src,
            report: &mut r,
            consumed: &mut c,
            rewrite: &rw,
        };
        let crews = crews::import(store.conn(), &mut cx).unwrap();
        sessions::import(store.conn(), &mut cx, &crews).unwrap();
        for c in &crews {
            whiteboard::import_board(store.conn(), &mut cx, &c.id).unwrap();
            todos::import(store.conn(), &mut cx, &c.id, crewd_proto::Ledger::Agent).unwrap();
            todos::import(store.conn(), &mut cx, &c.id, crewd_proto::Ledger::Human).unwrap();
        }
        whiteboard::import_outbox(store.conn(), &mut cx).unwrap();
        archive::import_rest(store.conn(), &mut cx).unwrap();
        r
    }

    #[test]
    fn messages_todos_and_archive_import_from_fixture() {
        let src = fixture();
        let store = crewd_store::Store::open_in_memory().unwrap();
        let r = import_parts(&src, &store);

        let msgs = crewd_store::messages::list(
            store.conn(),
            &crewd_store::channels::lobby_id("local-aaaa"),
            0,
            None,
            100,
        )
        .unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].text, "你好，大家 👋");
        assert_eq!(
            msgs[0].mentions[0].target_id.as_deref(),
            Some("worker-2222bbbb")
        );
        assert_eq!(msgs[1].created_at, "2026-09-11T04:35:00.250Z");
        assert_eq!(
            msgs[1].in_reply_to.as_deref(),
            Some("11111111-0000-4000-8000-000000000001")
        );
        assert_eq!(
            msgs[2].sender_kind,
            crewd_proto::SenderKind::Captain,
            "outbox 里的那条排在板上消息之后"
        );
        let extra: String = store
            .conn()
            .query_row(
                "SELECT extra FROM messages WHERE id='11111111-0000-4000-8000-000000000001'",
                [],
                |x| x.get(0),
            )
            .unwrap();
        assert!(extra.contains("\"audience\":\"agents\""));

        let agent =
            crewd_store::todos::list(store.conn(), "local-aaaa", crewd_proto::Ledger::Agent)
                .unwrap();
        assert_eq!(agent.len(), 2);
        assert!(agent[0].deleted_at.is_some());
        assert_eq!(agent[1].responses.len(), 1);
        let human =
            crewd_store::todos::list(store.conn(), "local-aaaa", crewd_proto::Ledger::Human)
                .unwrap();
        assert_eq!(human.len(), 1);

        let paths = crewd_store::archive::list_paths(store.conn()).unwrap();
        assert!(paths.contains(&"crew-arrangement.json".to_string()));
        assert!(paths.contains(&"whiteboards/local-aaaa.plan.json".to_string()));
        assert!(
            paths.contains(&"whiteboards/local-bbbb.json".to_string()),
            "坏文件原样归档"
        );
        assert!(!paths.iter().any(|p| p.ends_with("crew-sessions.json")));
        assert!(!paths.iter().any(|p| p.ends_with(".cursor")));
        assert!(!paths.iter().any(|p| p.starts_with("attachments/")));
        assert!(
            paths.contains(&"whiteboards/local-aaaa.json".to_string()),
            "有坏行（下标 2）的白板：好行照常导入，整份文件也原样归档"
        );
        assert_eq!(
            crewd_store::archive::get(store.conn(), "whiteboards/local-aaaa.json")
                .unwrap()
                .unwrap(),
            fs::read(src.join("whiteboards/local-aaaa.json")).unwrap()
        );
        assert_eq!(
            crewd_store::archive::get(store.conn(), "local-crews.json")
                .unwrap()
                .unwrap(),
            fs::read(src.join("local-crews.json")).unwrap(),
            "local-crews.json 总是原样归档"
        );
        assert!(
            paths.contains(&"whiteboards/agent-sessions.json".to_string()),
            "有落选（被更新记录取代）的 session 记录：整份原样归档，旧的 agentSessionId 不丢"
        );
        assert!(
            !paths
                .iter()
                .any(|p| p == "whiteboards/local-aaaa.todos.json"),
            "全部进库的账本不重复归档"
        );
        assert!(
            !paths.iter().any(|p| p.starts_with("whiteboards/outbox/")),
            "已导入的 outbox 文件不重复归档"
        );

        assert_eq!(r.counts["messages"], 3);
        assert_eq!(r.counts["outboxMessages"], 1);
        assert_eq!(r.counts["syntheticRowsDropped"], 1);
        assert_eq!(r.counts["agentTodos"], 2);
        assert_eq!(r.counts["humanTodos"], 1);
        assert_eq!(r.counts["todoResponses"], 1);
        assert_eq!(r.counts["attachmentPathsNotRewritten"], 1);
        assert!(
            r.skipped
                .iter()
                .any(|s| s.file == "whiteboards/local-aaaa.json" && s.index == Some(2))
        );
        assert!(
            r.skipped
                .iter()
                .any(|s| s.file == "whiteboards/local-bbbb.json" && s.index.is_none())
        );
    }

    #[test]
    fn importing_same_board_twice_is_idempotent_for_messages() {
        let src = fixture();
        let store = crewd_store::Store::open_in_memory().unwrap();
        let (mut r, mut c, rw) = ctx_parts(&src);
        let mut cx = Ctx {
            src: &src,
            report: &mut r,
            consumed: &mut c,
            rewrite: &rw,
        };
        crews::import(store.conn(), &mut cx).unwrap();
        whiteboard::import_board(store.conn(), &mut cx, "local-aaaa").unwrap();
        whiteboard::import_board(store.conn(), &mut cx, "local-aaaa").unwrap();
        assert_eq!(
            crewd_store::messages::list(
                store.conn(),
                &crewd_store::channels::lobby_id("local-aaaa"),
                0,
                None,
                100,
            )
            .unwrap()
            .len(),
            2
        );
        assert_eq!(r.counts["duplicateMessages"], 2);
    }

    #[test]
    fn rows_that_do_not_reach_the_store_keep_their_file_for_the_archive() {
        let crews = r#"{"crews":[{"id":"c1","title":"t","createdAt":"2026-09-01T00:00:00Z","updatedAt":"2026-09-01T00:00:00Z"}]}"#;
        // 一条 session 记录指向不存在的机组 → 这行进不了库。
        let recs = session_rec("worker-x", "nope", "a", "2026-09-01T09:00:00Z");
        let src = legacy_root(crews, &format!("[{recs}]"));
        // 同一个 id 出现两次、内容不同 → 第二行进不了库。
        let board = r#"[
            {"id":"m1","senderKind":"user","text":"first","createdAt":"2026-09-01T01:00:00Z"},
            {"id":"m1","senderKind":"user","text":"different","createdAt":"2026-09-01T02:00:00Z"}
        ]"#;
        fs::write(src.path().join("whiteboards/c1.json"), board).unwrap();
        let store = crewd_store::Store::open_in_memory().unwrap();
        let r = import_parts(src.path(), &store);
        assert_eq!(r.counts["messages"], 1);
        assert_eq!(r.counts["duplicateMessages"], 1);
        let paths = crewd_store::archive::list_paths(store.conn()).unwrap();
        assert!(
            paths.contains(&"whiteboards/c1.json".to_string()),
            "{paths:?}"
        );
        assert!(
            paths.contains(&"whiteboards/agent-sessions.json".to_string()),
            "{paths:?}"
        );
        assert!(paths.contains(&"local-crews.json".to_string()), "{paths:?}");
    }

    /// 裁定 6 + 控制者裁定 R14：认不出的 sender / mention 词不猜、不丢——行进报告的 skipped（带下标）并加警告，
    /// 整份文件原样归档；认得的旧词（user）归一后照常导入。
    #[test]
    fn rows_with_unknown_sender_or_mention_are_skipped_and_file_is_archived() {
        let crews = r#"{"crews":[{"id":"c1","title":"t","createdAt":"2026-09-01T00:00:00Z","updatedAt":"2026-09-01T00:00:00Z"}]}"#;
        let src = legacy_root(crews, "[]");
        fs::write(
            src.path().join("whiteboards/c1.json"),
            r#"[{"id":"m1","senderKind":"user","text":"示例消息","createdAt":"2026-09-01T01:00:00Z"},
                {"id":"m2","senderKind":"robot","text":"t","createdAt":"2026-09-01T01:01:00Z"},
                {"id":"m3","senderKind":"session","text":"t","createdAt":"2026-09-01T01:02:00Z",
                 "mentions":[{"kind":"everyone"}]}]"#,
        )
        .unwrap();
        let store = crewd_store::Store::open_in_memory().unwrap();
        let r = import_parts(src.path(), &store);
        let msgs = crewd_store::messages::list(
            store.conn(),
            &crewd_store::channels::lobby_id("c1"),
            0,
            None,
            100,
        )
        .unwrap();
        assert_eq!(
            msgs.iter()
                .map(|m| (m.id.as_str(), m.sender_kind.as_str()))
                .collect::<Vec<_>>(),
            [("m1", "human")]
        );
        assert_eq!(r.counts["messagesWithUnknownSender"], 1);
        assert_eq!(r.counts["messagesWithUnknownMention"], 1);
        let sk: Vec<_> = r
            .skipped
            .iter()
            .filter(|s| s.file == "whiteboards/c1.json")
            .map(|s| (s.index, s.reason.as_str()))
            .collect();
        assert_eq!(sk.len(), 2, "{sk:?}");
        assert_eq!(sk[0].0, Some(1));
        assert!(sk[0].1.contains("robot"), "{sk:?}");
        assert_eq!(sk[1].0, Some(2));
        assert!(sk[1].1.contains("everyone"), "{sk:?}");
        assert!(r.warnings.iter().any(|w| w.contains("robot")));
        assert!(r.warnings.iter().any(|w| w.contains("everyone")));
        let paths = crewd_store::archive::list_paths(store.conn()).unwrap();
        assert!(
            paths.contains(&"whiteboards/c1.json".to_string()),
            "{paths:?}"
        );
    }

    #[test]
    fn unmodeled_fields_inside_nested_objects_keep_their_file_for_the_archive() {
        let crews = r#"{"crews":[{"id":"c1","title":"t","createdAt":"2026-09-01T00:00:00Z","updatedAt":"2026-09-01T00:00:00Z"},
                                 {"id":"c2","title":"t","createdAt":"2026-09-01T00:00:00Z","updatedAt":"2026-09-01T00:00:00Z"}]}"#;
        let src = legacy_root(crews, "[]");
        // mention / attachment 里有没建模的键：行能进库，但这些键没地方放。
        fs::write(
            src.path().join("whiteboards/c1.json"),
            r#"[{"id":"m1","senderKind":"user","text":"t","createdAt":"2026-09-01T01:00:00Z",
                 "mentions":[{"kind":"session","target_id":"w","label":"@w"}]}]"#,
        )
        .unwrap();
        fs::write(
            src.path().join("whiteboards/c2.json"),
            r#"[{"id":"m2","senderKind":"user","text":"t","createdAt":"2026-09-01T01:00:00Z"}]"#,
        )
        .unwrap();
        fs::write(
            src.path().join("whiteboards/c1.todos.json"),
            r#"[{"id":"t1","number":1,"text":"x","status":"open","createdAt":"2026-09-01T01:00:00Z",
                 "responses":[{"id":"r1","sessionId":"w","text":"ok","createdAt":"2026-09-01T02:00:00Z","reaction":"👍"}]}]"#,
        )
        .unwrap();
        fs::write(
            src.path().join("whiteboards/c2.todos.json"),
            r#"[{"id":"t2","number":1,"text":"x","status":"open","createdAt":"2026-09-01T01:00:00Z",
                 "attachments":[{"id":"a","mime":"x","path":"/p","width":3}]}]"#,
        )
        .unwrap();
        let store = crewd_store::Store::open_in_memory().unwrap();
        let r = import_parts(src.path(), &store);
        assert_eq!(r.counts["messages"], 2);
        assert_eq!(r.counts["agentTodos"], 2);
        assert_eq!(r.counts["rowsWithUnmodeledNestedFields"], 3);
        let paths = crewd_store::archive::list_paths(store.conn()).unwrap();
        for p in [
            "whiteboards/c1.json",
            "whiteboards/c1.todos.json",
            "whiteboards/c2.todos.json",
        ] {
            assert!(paths.contains(&p.to_string()), "{p}: {paths:?}");
        }
        assert!(
            !paths.contains(&"whiteboards/c2.json".to_string()),
            "全建模的文件不重复归档"
        );
    }

    #[test]
    fn duplicate_todo_number_is_skipped_by_index_and_ledger_file_is_archived() {
        let crews = r#"{"crews":[{"id":"c1","title":"t","createdAt":"2026-09-01T00:00:00Z","updatedAt":"2026-09-01T00:00:00Z"}]}"#;
        let src = legacy_root(crews, "[]");
        let todos_json = r#"[
            {"id":"t-a","number":1,"text":"first","status":"open","createdAt":"2026-09-01T01:00:00Z"},
            {"id":"t-b","number":1,"text":"same number","status":"open","createdAt":"2026-09-01T02:00:00Z",
             "responses":[{"id":"r-b","sessionId":"worker-1","text":"done","createdAt":"2026-09-01T03:00:00Z"}]}
        ]"#;
        fs::write(src.path().join("whiteboards/c1.todos.json"), todos_json).unwrap();
        let store = crewd_store::Store::open_in_memory().unwrap();
        let (mut r, mut c, rw) = ctx_parts(src.path());
        let mut cx = Ctx {
            src: src.path(),
            report: &mut r,
            consumed: &mut c,
            rewrite: &rw,
        };
        let imported = crews::import(store.conn(), &mut cx).unwrap();
        todos::import(store.conn(), &mut cx, "c1", crewd_proto::Ledger::Agent).unwrap();
        archive::import_rest(store.conn(), &mut cx).unwrap();

        let list =
            crewd_store::todos::list(store.conn(), &imported[0].id, crewd_proto::Ledger::Agent)
                .unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "t-a");
        assert_eq!(r.counts["agentTodos"], 1);
        assert_eq!(r.counts.get("todoResponses").copied().unwrap_or(0), 0);
        let sk: Vec<_> = r
            .skipped
            .iter()
            .filter(|s| s.file == "whiteboards/c1.todos.json")
            .collect();
        assert_eq!(sk.len(), 1);
        assert_eq!(sk[0].index, Some(1));
        assert!(sk[0].reason.contains("#1") && sk[0].reason.contains("t-b"));
        let orphans: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM todo_responses", [], |x| x.get(0))
            .unwrap();
        assert_eq!(orphans, 0, "被跳过的 todo 不留下任何 response");

        let paths = crewd_store::archive::list_paths(store.conn()).unwrap();
        assert!(paths.contains(&"whiteboards/c1.todos.json".to_string()));
        let raw = crewd_store::archive::get(store.conn(), "whiteboards/c1.todos.json")
            .unwrap()
            .unwrap();
        assert_eq!(raw, todos_json.as_bytes(), "原样归档");
    }
}
