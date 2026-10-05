//! 把旧 Swift 版的 JSON 数据只读地导入 crewd 的 SQLite。源目录一个字节都不改。
pub mod paths;
// used from Task 6-8
#[allow(dead_code)]
mod model;
mod report;
// used from Task 6-8
#[allow(dead_code)]
mod rows;

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
