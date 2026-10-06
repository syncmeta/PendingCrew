//! `crewd claude` 的胶水：库里的会话号账本 → 第一次新号、第二次 `--resume` 同一个号。
//! 「claude」用 shell 脚本顶替（打印参数后退出）。
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use crewd_proto::{Crew, Session};
use crewd_store::{Store, crews, sessions};

fn seed(data: &Path) {
    let s = Store::open(&data.join("crewd.db")).unwrap();
    crews::upsert(
        s.conn(),
        &Crew {
            id: "c".into(),
            title: "机组".into(),
            number: None,
            builtin: false,
            parent_ids: vec![],
            working_directory: None,
            captain_executor: None,
            hidden_at: None,
            created_at: "2026-10-01T00:00:00.000Z".into(),
            updated_at: "2026-10-01T00:00:00.000Z".into(),
        },
        None,
    )
    .unwrap();
    sessions::upsert(
        s.conn(),
        &Session {
            id: "w-1".into(),
            crew_id: "c".into(),
            role: "worker".into(),
            executor: "claude".into(),
            agent_session_id: None,
            display_name: None,
            working_directory: None,
            model: None,
            effort: None,
            updated_at: "2026-10-01T00:00:00.000Z".into(),
        },
        None,
    )
    .unwrap();
}

fn run(data: &Path, fake: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_crewd"))
        .env("CREWD_DATA_DIR", data)
        // 登录 shell 抓 PATH 走失败分支：测试不依赖本机的 rc 文件。
        .env("SHELL", "/usr/bin/false")
        .args(["claude", "--session", "w-1", "--workdir"])
        .arg(data)
        .arg("--claude")
        .arg(fake)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn second_run_resumes_the_id_recorded_by_the_first() {
    let data = tempfile::tempdir().unwrap();
    seed(data.path());
    let fake = data.path().join("fake-claude");
    std::fs::write(&fake, "#!/bin/sh\necho \"ARGS $*\"\nsleep 0.2\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    let first = run(data.path(), &fake);
    assert!(first.contains("resumed: false"), "{first}");
    assert!(first.contains("Exited("), "{first}");
    let recorded = {
        let s = Store::open(&data.path().join("crewd.db")).unwrap();
        sessions::agent_session_id(s.conn(), "w-1")
            .unwrap()
            .expect("第一次起之前就该落账")
    };
    assert!(first.contains(&recorded), "{first}");

    let second = run(data.path(), &fake);
    assert!(
        second.contains(&format!("agent_session_id: \"{recorded}\", resumed: true")),
        "{second}"
    );
    let registry = std::fs::read_to_string(data.path().join("exec-registry.json")).unwrap();
    assert_eq!(registry.trim(), "{}", "退出后登记表清空");
}
