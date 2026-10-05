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

/// 把共享夹具复制到临时目录，测试可以随便折腾副本而不碰夹具本身。
fn fixture_copy() -> tempfile::TempDir {
    fn cp(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let p = e.unwrap().path();
            let t = to.join(p.file_name().unwrap());
            if p.is_dir() {
                cp(&p, &t);
            } else {
                std::fs::copy(&p, &t).unwrap();
            }
        }
    }
    let t = tempfile::tempdir().unwrap();
    cp(&fixture(), t.path());
    t
}

#[test]
fn overlapping_source_and_data_dir_is_refused_before_anything_is_written() {
    let t = fixture_copy();
    let before = snapshot(t.path());
    for dest in [t.path().to_path_buf(), t.path().join("crewd")] {
        let mut store = Store::open_in_memory().unwrap();
        let err = import(t.path(), &mut store, &dest).unwrap_err();
        assert!(err.to_string().contains("重叠"), "{dest:?}: {err}");
        assert!(store.is_empty().unwrap());
        assert_eq!(
            std::fs::read_to_string(t.path().join("attachments/local-aaaa/note.txt")).unwrap(),
            "hello"
        );
        assert_eq!(
            snapshot(t.path()),
            before,
            "{dest:?}: 源目录不能多出或改动任何文件"
        );
        assert!(!t.path().join("crewd").exists());
    }
    // 反过来：源目录在数据目录里面。
    let mut store = Store::open_in_memory().unwrap();
    let err = import(&t.path().join("whiteboards"), &mut store, t.path()).unwrap_err();
    assert!(err.to_string().contains("重叠"), "{err}");
    assert_eq!(snapshot(t.path()), before);
}

#[test]
fn check_disjoint_resolves_dots_and_missing_paths() {
    use crewd_legacy::paths::check_disjoint;
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("legacy");
    std::fs::create_dir_all(&src).unwrap();
    assert!(check_disjoint(&src, &t.path().join("crewd")).is_ok());
    assert!(
        check_disjoint(&src, &t.path().join("legacy-2")).is_ok(),
        "前缀相同不算重叠"
    );
    assert!(check_disjoint(&src, &src.join("a/b/c")).is_err());
    assert!(check_disjoint(&src, &src.join("x/..")).is_err());
    assert!(check_disjoint(&src.join("."), &src).is_err());
    assert!(check_disjoint(&src, t.path()).is_err());
    // 大小写不敏感的文件系统（macOS 默认）上，换个大小写还是同一个目录。
    if t.path().join("LEGACY").exists() {
        assert!(check_disjoint(&src, &t.path().join("LEGACY/crewd")).is_err());
    }
}
