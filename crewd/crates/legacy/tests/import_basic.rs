use crewd_legacy::import;
use crewd_store::Store;
use std::path::Path;

fn fixture() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/basic")
}

fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = vec![];
    fn walk(root: &Path, d: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                out.push((
                    p.strip_prefix(root).unwrap().display().to_string(),
                    std::fs::read(&p).unwrap(),
                ));
            }
        }
    }
    walk(dir, dir, &mut out);
    out.sort();
    out
}

#[test]
fn full_import_from_fixture() {
    let before = snapshot(&fixture());
    let dest = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dest.path().join("crewd.db")).unwrap();
    let r = import(&fixture(), &mut store, dest.path()).unwrap();

    assert_eq!(r.counts["crews"], 3);
    assert_eq!(r.counts["sessions"], 3);
    assert_eq!(r.counts["messages"], 3);
    assert_eq!(r.counts["agentTodos"], 2);
    assert_eq!(r.counts["humanTodos"], 1);
    assert_eq!(r.counts["attachmentFiles"], 1);
    assert!(dest.path().join("attachments/local-aaaa/note.txt").exists());
    assert!(
        crewd_store::meta_get(store.conn(), "importedAt")
            .unwrap()
            .is_some()
    );
    assert_eq!(snapshot(&fixture()), before, "源目录必须一个字节都不变");
}

#[test]
fn missing_source_is_an_error() {
    let dest = tempfile::tempdir().unwrap();
    let mut store = Store::open_in_memory().unwrap();
    assert!(import(&dest.path().join("nope"), &mut store, dest.path()).is_err());
    assert!(store.is_empty().unwrap());
}

#[test]
fn non_empty_store_is_rejected_and_nothing_is_added() {
    let dest = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dest.path().join("crewd.db")).unwrap();
    import(&fixture(), &mut store, dest.path()).unwrap();
    let count = |s: &Store| -> i64 {
        s.conn()
            .query_row(
                "SELECT (SELECT COUNT(*) FROM crews) + (SELECT COUNT(*) FROM messages) + (SELECT COUNT(*) FROM sessions)",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    let before = count(&store);
    let err = import(&fixture(), &mut store, dest.path()).unwrap_err();
    assert!(err.to_string().contains("不是空的"), "{err}");
    assert_eq!(count(&store), before);
}

#[test]
fn attachment_copy_failure_commits_nothing() {
    let dest = tempfile::tempdir().unwrap();
    // 目标 attachments 路径被一个普通文件占住 → 复制必失败。
    std::fs::write(dest.path().join("attachments"), "in the way").unwrap();
    let mut store = Store::open(&dest.path().join("crewd.db")).unwrap();
    let err = import(&fixture(), &mut store, dest.path()).unwrap_err();
    assert!(err.to_string().contains("复制附件失败"), "{err}");
    assert!(store.is_empty().unwrap(), "复制失败不能留下任何行");
    assert!(
        crewd_store::meta_get(store.conn(), "importedAt")
            .unwrap()
            .is_none()
    );
}
