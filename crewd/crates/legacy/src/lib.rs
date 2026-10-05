//! 把旧 Swift 版的 JSON 数据只读地导入 crewd 的 SQLite。源目录一个字节都不改。
// used from Task 8
#[allow(dead_code)]
mod crews;
pub mod paths;
// used from Task 6-8
#[allow(dead_code)]
mod model;
mod report;
// used from Task 6-8
#[allow(dead_code)]
mod rows;
// used from Task 8
#[allow(dead_code)]
mod sessions;

pub use report::{ImportReport, Skipped};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

// used from Task 6-8
#[allow(dead_code)]
pub(crate) struct Ctx<'a> {
    pub src: &'a Path,
    pub report: &'a mut ImportReport,
    pub consumed: &'a mut BTreeSet<PathBuf>,
    pub rewrite: &'a paths::PathRewrite,
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
        let rows: Vec<model::LAttachment> = rows::read_rows(&mut cx, &good);
        assert_eq!(rows.len(), 2);
        assert!(rows::read_rows::<model::LAttachment>(&mut cx, &bad).is_empty());
        assert!(rows::read_rows::<model::LAttachment>(&mut cx, &obj).is_empty());
        assert!(
            rows::read_rows::<model::LAttachment>(&mut cx, &dir.path().join("missing.json"))
                .is_empty()
        );
        assert_eq!(r.skipped.len(), 3);
        assert_eq!(r.skipped[0].file, "good.json");
        assert_eq!(r.skipped[0].index, Some(1));
        assert!(c.contains(&good));
        assert!(!c.contains(&bad), "坏文件不算 consumed，留给归档原样保存");
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
        assert!(chief.builtin);
        assert_eq!(list[0].created_at, "2026-09-01T08:00:00.000Z");

        let ses = crewd_store::sessions::list_for_crew(store.conn(), "local-aaaa").unwrap();
        assert_eq!(ses.len(), 3);
        let cap = ses.iter().find(|s| s.id == "captain-1111aaaa").unwrap();
        assert_eq!(cap.role, "captain");
        assert_eq!(cap.display_name.as_deref(), Some("机长"));
        let w = ses.iter().find(|s| s.id == "worker-2222bbbb").unwrap();
        assert_eq!(
            w.agent_session_id.as_deref(),
            Some("thread-xyz"),
            "取 updatedAt 最新的那条"
        );
        assert_eq!(w.model.as_deref(), Some("gpt-5.6"));
        let old = ses.iter().find(|s| s.id == "worker-3333cccc").unwrap();
        assert_eq!(old.agent_kind, "unknown");

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
}
