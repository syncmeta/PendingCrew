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
    let stored: serde_json::Value = serde_json::from_str(
        &crewd_store::meta_get(store.conn(), "importReport")
            .unwrap()
            .expect("导入报告存进 meta.importReport"),
    )
    .unwrap();
    assert_eq!(stored["counts"]["messages"], 3);
    assert_eq!(stored, serde_json::to_value(&r).unwrap());
    assert_eq!(snapshot(&fixture()), before, "源目录必须一个字节都不变");
}

#[test]
fn imported_database_has_lobbies_pinned_cursors_and_counters() {
    let dest = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dest.path().join("crewd.db")).unwrap();
    import(&fixture(), &mut store, dest.path()).unwrap();
    let c = store.conn();
    let lobbies: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM channels WHERE kind='lobby'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let crews: i64 = c
        .query_row("SELECT COUNT(*) FROM crews", [], |r| r.get(0))
        .unwrap();
    assert_eq!(lobbies, crews);
    let behind: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM sessions s JOIN read_cursors r
               ON r.reader=s.id AND r.channel_id=s.crew_id||'/lobby'
              WHERE r.seq < (SELECT COALESCE(MAX(seq),0) FROM messages m WHERE m.channel_id=s.crew_id||'/lobby')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(behind, 0, "导入后没有任何 session 有未读");
    assert_eq!(
        crewd_store::meta_get(c, "nextCrewNumber")
            .unwrap()
            .as_deref(),
        Some("3"),
        "夹具 local-crews.json 的 nextCrewNumber"
    );
    let ext: i64 = c
        .query_row(
            "SELECT ext FROM sessions WHERE id='worker-2222bbbb'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ext, 2);
    let cap: String = c
        .query_row(
            "SELECT captain_session_id FROM crews WHERE id='local-aaaa'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cap, "captain-1111aaaa");
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

/// 测试结束（含 panic）时把权限改回去，临时目录才删得掉。
struct RestoreMode(std::path::PathBuf, u32);
impl Drop for RestoreMode {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(self.1));
    }
}

#[test]
fn unreadable_legacy_file_aborts_the_import_and_leaves_the_store_empty() {
    use std::os::unix::fs::PermissionsExt;
    for rel in [
        "local-crews.json",
        "whiteboards/agent-sessions.json",
        "whiteboards/local-aaaa.json",
        "whiteboards/local-aaaa.todos.json",
        "whiteboards/outbox/local-aaaa.1757594300000.1.11111111-0000-4000-8000-000000000004.json",
        "whiteboards/outbox",
        "crew-arrangement.json",
        "whiteboards",
    ] {
        let t = fixture_copy();
        let target = t.path().join(rel);
        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o000)).unwrap();
        let _restore = RestoreMode(target.clone(), mode);
        let probe = if target.is_dir() {
            std::fs::read_dir(&target).map(|_| ())
        } else {
            std::fs::read(&target).map(|_| ())
        };
        if probe.is_ok() {
            eprintln!("以 root 运行，权限挡不住读取：跳过");
            return;
        }
        let dest = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dest.path().join("crewd.db")).unwrap();
        let err = import(t.path(), &mut store, dest.path()).unwrap_err();
        assert!(format!("{err:#}").contains(rel), "{rel}: {err:#}");
        assert!(store.is_empty().unwrap(), "{rel}: I/O 失败必须整体回滚");
        let rows: i64 = store
            .conn()
            .query_row(
                "SELECT (SELECT COUNT(*) FROM sessions) + (SELECT COUNT(*) FROM todos) + (SELECT COUNT(*) FROM legacy_archive) + (SELECT COUNT(*) FROM meta WHERE key LIKE 'import%')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 0, "{rel}: 回滚后可以重试");
    }
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let p = e.unwrap().path();
        let dest = to.join(p.file_name().unwrap());
        if p.is_dir() {
            copy_tree(&p, &dest);
        } else {
            std::fs::copy(&p, &dest).unwrap();
        }
    }
}

/// Swift 时代的 captain mention 没有 target：导入时和迁移走同一条回填，落成该机组的机长指针。
#[test]
fn legacy_captain_mentions_without_target_are_resolved_by_the_import() {
    let src = tempfile::tempdir().unwrap();
    copy_tree(&fixture(), src.path());
    std::fs::write(
        src.path().join("whiteboards/local-aaaa.json"),
        r#"[{"id":"22222222-0000-4000-8000-000000000001","senderKind":"session","senderSessionId":"worker-2222bbbb",
             "text":"请机长看","createdAt":"2026-09-11T12:34:56Z","mentions":[{"kind":"captain"}]}]"#,
    )
    .unwrap();
    let dest = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dest.path().join("crewd.db")).unwrap();
    import(src.path(), &mut store, dest.path()).unwrap();
    let m = crewd_store::messages::list(store.conn(), "local-aaaa/lobby", 0, None, 10).unwrap();
    let hit = m
        .iter()
        .find(|m| m.id == "22222222-0000-4000-8000-000000000001")
        .expect("消息已导入");
    assert_eq!(hit.mentions.len(), 1);
    assert_eq!(
        hit.mentions[0].target_id.as_deref(),
        Some("captain-1111aaaa")
    );
}
