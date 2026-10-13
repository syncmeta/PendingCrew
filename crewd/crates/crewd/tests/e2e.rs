use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::time::{Duration, Instant};

// Each binary test process owns a synthetic home outside every data directory.
static TEST_HOME: std::sync::LazyLock<tempfile::TempDir> =
    std::sync::LazyLock::new(|| tempfile::tempdir().unwrap());

fn crewd(data: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_crewd"));
    c.env("CREWD_DATA_DIR", data)
        .env_remove("PENDINGCREW_DATA_DIR")
        .env("CREWD_CLAUDE", "/usr/bin/false")
        .env_remove("CREWD_CODEX")
        .env("SHELL", "/usr/bin/false")
        .env("HOME", TEST_HOME.path())
        .env("XDG_CACHE_HOME", TEST_HOME.path().join("cache"))
        .env("CLAUDE_CONFIG_DIR", TEST_HOME.path().join(".claude"));
    c
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../legacy/tests/fixtures/basic")
}

struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start(data: &Path) -> Daemon {
    let d = Daemon(crewd(data).arg("serve").spawn().unwrap());
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(10) {
        if crewd(data)
            .args(["rpc", "system.hello"])
            .output()
            .unwrap()
            .status
            .success()
        {
            return d;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("crewd serve 没起来");
}

fn rpc(data: &Path, method: &str, params: &str) -> serde_json::Value {
    let out: Output = crewd(data).args(["rpc", method, params]).output().unwrap();
    assert!(
        out.status.success(),
        "{method}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn import_then_serve_then_rpc() {
    let data = tempfile::tempdir().unwrap();
    let out = crewd(data.path())
        .args(["import", "--from"])
        .arg(fixture())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["counts"]["messages"], 3);

    let again = crewd(data.path())
        .args(["import", "--from"])
        .arg(fixture())
        .output()
        .unwrap();
    assert!(!again.status.success(), "已有库时必须拒绝导入");
    // 复审 1：只挪 crewd.db 会被身份核对拦住（crewd.db.id 还在），提示里要说连它一起挪。
    let hint = String::from_utf8_lossy(&again.stderr);
    assert!(hint.contains("crewd.db.id"), "{hint}");

    let _d = start(data.path());
    let crews = rpc(data.path(), "crew.list", "null");
    assert_eq!(crews.as_array().unwrap().len(), 3);
    let before = rpc(
        data.path(),
        "message.list",
        r#"{"channelId":"local-aaaa/lobby"}"#,
    );
    assert_eq!(before.as_array().unwrap().len(), 3);
    rpc(
        data.path(),
        "message.post",
        r#"{"channelId":"local-aaaa/lobby","text":"切换后第一条 ✅"}"#,
    );
    let after = rpc(
        data.path(),
        "message.list",
        r#"{"channelId":"local-aaaa/lobby"}"#,
    );
    assert_eq!(after[3]["text"], "切换后第一条 ✅");
    assert_eq!(after[3]["channelId"], "local-aaaa/lobby");
    assert_eq!(after[3]["senderKind"], "human");

    let second = crewd(data.path()).arg("serve").output().unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("另一个 crewd"));
}

/// 夹具的可写副本（共享夹具本身不许动）。
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

fn list_tree(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = vec![];
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            out.extend(list_tree(&p));
        } else {
            let b = std::fs::read(&p).unwrap();
            out.push((p, b));
        }
    }
    out.sort();
    out
}

#[test]
fn import_into_the_legacy_dir_itself_is_refused_and_writes_nothing() {
    let legacy = fixture_copy();
    let before = list_tree(legacy.path());
    let out = crewd(legacy.path())
        .args(["import", "--from"])
        .arg(legacy.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("重叠"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!legacy.path().join("crewd.db").exists());
    assert!(!legacy.path().join("crewd.lock").exists());
    assert_eq!(list_tree(legacy.path()), before, "旧目录一个字节都不能变");
}

#[test]
fn import_is_refused_while_serve_holds_the_data_dir() {
    let data = tempfile::tempdir().unwrap();
    let _d = start(data.path());
    let out = crewd(data.path())
        .args(["import", "--from"])
        .arg(fixture())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("另一个 crewd"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let crews = rpc(data.path(), "crew.list", "null");
    assert_eq!(crews.as_array().unwrap().len(), 0, "serve 的库没被动过");
}

#[test]
fn fresh_data_dir_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("nested/crewd");
    let out = crewd(&data)
        .args(["import", "--from"])
        .arg(fixture())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mode = std::fs::metadata(&data).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700, "{mode:o}");
}

#[test]
fn data_dir_is_trimmed_and_tilde_expanded() {
    let home = tempfile::tempdir().unwrap();
    for (how, raw, rel) in [
        ("env", "  ~/from-env \n", "from-env"),
        ("flag", " ~/from-flag ", "from-flag"),
    ] {
        let mut c = Command::new(env!("CARGO_BIN_EXE_crewd"));
        c.env("HOME", home.path())
            .env_remove("PENDINGCREW_DATA_DIR")
            .env("CREWD_CLAUDE", "/usr/bin/false")
            .env("SHELL", "/usr/bin/false")
            .env("XDG_CACHE_HOME", TEST_HOME.path().join("cache"))
            .env("CLAUDE_CONFIG_DIR", TEST_HOME.path().join(".claude"));
        if how == "env" {
            c.env("CREWD_DATA_DIR", raw);
        } else {
            c.env_remove("CREWD_DATA_DIR").args(["--data-dir", raw]);
        }
        let out = c
            .args(["import", "--from"])
            .arg(fixture())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{how}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(home.path().join(rel).join("crewd.db").exists(), "{how}");
    }
}

#[test]
fn existing_empty_db_does_not_block_import() {
    let data = tempfile::tempdir().unwrap();
    // 例如上次导入失败、或 serve 先跑过一次：库文件在，但里面什么都没有。
    drop(crewd_store::Store::open(&data.path().join("crewd.db")).unwrap());
    let out = crewd(data.path())
        .args(["import", "--from"])
        .arg(fixture())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["counts"]["messages"], 3);
}

#[test]
fn rpc_error_exits_nonzero() {
    let data = tempfile::tempdir().unwrap();
    let _d = start(data.path());
    let out = crewd(data.path())
        .args(["rpc", "no.such"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("未知方法"));
}

#[test]
fn committed_schema_matches_generated() {
    let generated = Command::new(env!("CARGO_BIN_EXE_crewd"))
        .arg("schema")
        .output()
        .unwrap();
    assert!(generated.status.success());
    let committed = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema/crewd-api.schema.json"),
    )
    .expect("缺 crewd/schema/crewd-api.schema.json，运行 `cargo run -p crewd -- schema > schema/crewd-api.schema.json`");
    let a: serde_json::Value = serde_json::from_slice(&generated.stdout).unwrap();
    let b: serde_json::Value = serde_json::from_slice(&committed).unwrap();
    assert_eq!(
        a, b,
        "API 契约变了：重新生成 schema/crewd-api.schema.json 并提交"
    );
}

/// 每个客户端方法都能经 `crewd rpc` 走到处理函数（不是 -32601）：能成功的就要求成功；需要 agent 先造数据的
/// （回答问、重启询问）用不存在的号，要求得到「不存在 / 规则不过」而不是「没有这个方法」。
#[test]
fn rpc_drives_every_client_method() {
    let data = tempfile::tempdir().unwrap();
    let out = crewd(data.path())
        .args(["import", "--from"])
        .arg(fixture())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _d = start(data.path());
    enum Want {
        Ok,
        Code(i32),
    }
    use Want::*;
    let cases: Vec<(&str, &str, Want)> = vec![
        ("system.hello", "null", Ok),
        ("crew.list", "null", Ok),
        ("crew.get", r#"{"crewId":"local-aaaa"}"#, Ok),
        (
            "crew.create",
            r#"{"title":"端到端","workingDirectory":"/tmp/example"}"#,
            Ok,
        ),
        (
            "crew.update",
            r#"{"crewId":"local-bbbb","statusLine":"跑端到端"}"#,
            Ok,
        ),
        (
            "crew.adopt",
            r#"{"crewId":"local-bbbb","parentId":"local-aaaa"}"#,
            Ok,
        ),
        (
            "crew.reorder",
            r#"{"parentId":"local-aaaa","crewIds":["nope"]}"#,
            Code(-32602),
        ),
        (
            "crew.release",
            r#"{"crewId":"local-bbbb","fromParentId":"local-aaaa","toParentId":"local-aaaa"}"#,
            Ok,
        ),
        ("crew.remove", r#"{"crewId":"nope"}"#, Code(-32004)),
        ("crew.restore", r#"{"crewId":"nope"}"#, Code(-32004)),
        ("crew.purge", r#"{"crewId":"nope"}"#, Code(-32004)),
        // 导入的顶层机组仍在顶层（总机组不在树里），人类群是它自己的（home_of，N1-R2）。
        ("channel.list", r#"{"crewId":"local-aaaa"}"#, Ok),
        // 总机组自己的管理用人类群：同一条懒建规则（list 建出来，update 才找得到）。
        ("channel.list", r#"{"crewId":"pendingcrew-chief"}"#, Ok),
        (
            "channel.create",
            r#"{"crewId":"local-aaaa","kind":"topic","name":"端到端群"}"#,
            Ok,
        ),
        (
            "channel.update",
            r#"{"channelId":"local-aaaa/human","purpose":"找人"}"#,
            Ok,
        ),
        (
            "channel.update",
            r#"{"channelId":"pendingcrew-chief/human","purpose":"管理"}"#,
            Ok,
        ),
        (
            "channel.addMembers",
            r#"{"channelId":"local-aaaa/human","members":[{"kind":"session","id":"worker-2222bbbb"}]}"#,
            Ok,
        ),
        (
            "channel.removeMember",
            r#"{"channelId":"local-aaaa/human","member":{"kind":"session","id":"worker-2222bbbb"}}"#,
            Ok,
        ),
        (
            "channel.watch",
            r#"{"channelId":"local-aaaa/lobby","on":true}"#,
            Ok,
        ),
        (
            "channel.setOwner",
            r#"{"channelId":"local-aaaa/lobby","owner":{"kind":"human"}}"#,
            Code(-32602),
        ),
        ("message.list", r#"{"channelId":"local-aaaa/lobby"}"#, Ok),
        (
            "message.post",
            r#"{"channelId":"local-aaaa/lobby","text":"切换后第一条 ✅"}"#,
            Ok,
        ),
        ("message.search", r#"{"query":"切换后"}"#, Ok),
        (
            "read.mark",
            r#"{"channelId":"local-aaaa/lobby","seq":1}"#,
            Ok,
        ),
        ("session.list", "null", Ok),
        ("session.get", r#"{"sessionId":"captain-1111aaaa"}"#, Ok),
        ("session.usage", r#"{"sessionId":"captain-1111aaaa"}"#, Ok),
        (
            "work.create",
            r#"{"crewId":"local-aaaa","title":"synthetic plan"}"#,
            Ok,
        ),
        (
            "work.update",
            r#"{"id":"W999","expectedRevision":"1","status":"done"}"#,
            Code(-32004),
        ),
        (
            "work.reassignLost",
            r#"{"id":"W999","expectedRevision":"1","owner":"worker-2222bbbb","note":"loss"}"#,
            Code(-32004),
        ),
        (
            "work.transferRequest",
            r#"{"id":"W999","expectedRevision":"1","to":"worker-2222bbbb","note":"handoff"}"#,
            Code(-32004),
        ),
        (
            "work.transferAccept",
            r#"{"id":"W999","expectedRevision":"1"}"#,
            Code(-32004),
        ),
        (
            "work.transferWithdraw",
            r#"{"id":"W999","expectedRevision":"1"}"#,
            Code(-32004),
        ),
        (
            "work.assign",
            r#"{"id":"W999","expectedRevision":"1","owner":"synthetic-owner","note":"assign reason"}"#,
            Code(-32004),
        ),
        (
            "work.block",
            r#"{"id":"W999","expectedRevision":"1","on":[{"type":"work","id":"W998"}],"canContinue":{"type":"nothing"}}"#,
            Code(-32004),
        ),
        ("work.history", r#"{"id":"W999"}"#, Code(-32004)),
        ("work.list", "{}", Ok),
        ("work.get", r#"{"id":"W999"}"#, Code(-32004)),
        ("work.patrol", r#"{"crewId":"local-aaaa","limit":1}"#, Ok),
        ("ask.list", "null", Ok),
        // R11（控制者裁定 h）：号码格式对、夹具的账本里没有 = 不存在（-32004），不是参数错。
        (
            "ask.answer",
            r#"{"crewId":"local-aaaa","ask":"H1","text":"x"}"#,
            Code(-32004),
        ),
        (
            "ask.withdraw",
            r#"{"crewId":"local-aaaa","ask":"H1","reason":"x"}"#,
            Code(-32004),
        ),
        ("restart.pending", "null", Ok),
        (
            "restart.answer",
            r#"{"id":99,"answer":"later"}"#,
            Code(-32004),
        ),
        ("directory.list", "null", Ok),
        (
            "todo.create",
            r#"{"requestId":"9f75cfe6-ef21-4c90-8d09-4ea8f03903e8","crewId":"local-aaaa","text":"  CLI source\n原文不变  "}"#,
            Ok,
        ),
        ("todo.get", r##"{"id":"#999"}"##, Code(-32004)),
        (
            "todo.close",
            r##"{"id":"#999","note":"not found ruling","expectedStatus":"unclaimed"}"##,
            Code(-32004),
        ),
        (
            "todo.list",
            r#"{"crewId":"local-aaaa","ledger":"agent"}"#,
            Ok,
        ),
        (
            "document.create",
            r#"{"requestId":"fb69c781-91bb-4220-ae23-79d1a231861d","crewId":"local-aaaa","title":"CLI doc","body":"CLI original 🙂"}"#,
            Ok,
        ),
        (
            "document.get",
            r#"{"id":"11111111-1111-4111-8111-111111111111"}"#,
            Code(-32004),
        ),
        ("document.list", r#"{"crewId":"local-aaaa"}"#, Ok),
        (
            "document.update",
            r#"{"id":"11111111-1111-4111-8111-111111111111","expectedRevision":"1","title":"x","body":"x"}"#,
            Code(-32004),
        ),
        (
            "document.delete",
            r#"{"id":"11111111-1111-4111-8111-111111111111","expectedRevision":"1"}"#,
            Code(-32004),
        ),
        (
            "viewFolder.list",
            r#"{"crewId":"local-aaaa","view":"documents"}"#,
            Ok,
        ),
        (
            "viewFolder.create",
            r#"{"requestId":"56fc7f71-3fde-494b-a163-7c1a646af4af","crewId":"local-aaaa","view":"documents","expectedTreeRevision":"0","name":"CLI folder"}"#,
            Ok,
        ),
        (
            "viewFolder.update",
            r#"{"crewId":"local-aaaa","view":"progress","id":"11111111-1111-4111-8111-111111111111","expectedTreeRevision":"0","name":"x","position":0}"#,
            Code(-32004),
        ),
        (
            "viewFolder.delete",
            r#"{"crewId":"local-aaaa","view":"progress","id":"11111111-1111-4111-8111-111111111111","expectedTreeRevision":"0"}"#,
            Code(-32004),
        ),
        (
            "viewFolder.assign",
            r#"{"crewId":"local-aaaa","view":"progress","expectedTreeRevision":"0","object":{"kind":"work","id":"W999"}}"#,
            Code(-32003),
        ),
        (
            "channel.workList",
            r#"{"channelId":"local-aaaa/lobby"}"#,
            Ok,
        ),
        (
            "channel.workLink",
            r#"{"channelId":"local-aaaa/lobby","expectedLinkRevision":"0","object":{"kind":"work","id":"W999"}}"#,
            Code(-32004),
        ),
        (
            "channel.workUnlink",
            r#"{"channelId":"local-aaaa/lobby","expectedLinkRevision":"0","object":{"kind":"work","id":"W999"}}"#,
            Code(-32004),
        ),
        (
            "ask.get",
            r#"{"crewId":"local-aaaa","ask":"H999"}"#,
            Code(-32004),
        ),
        ("events.subscribe", r#"{"topics":["message"]}"#, Ok),
        // CLI serve does not enable Codex without --codex/CREWD_CODEX.
        // This offline fixture must never discover a real account runner.
        (
            "session.start",
            r#"{"crewId":"local-aaaa","role":"worker","executor":"codex","brief":"x","isolation":false}"#,
            Code(-32009),
        ),
        (
            "session.stop",
            r#"{"sessionId":"worker-2222bbbb","reason":"x"}"#,
            Ok,
        ),
        (
            "session.resume",
            r#"{"sessionId":"worker-2222bbbb"}"#,
            Code(-32009),
        ),
        ("session.live", r#"{"sessionId":"worker-2222bbbb"}"#, Ok),
        (
            "crew.setCaptain",
            r#"{"crewId":"local-aaaa","sessionId":"worker-2222bbbb"}"#,
            Ok,
        ),
        ("session.inspect", r#"{"sessionId":"worker-2222bbbb"}"#, Ok),
        (
            "session.answer",
            r#"{"sessionId":"worker-2222bbbb","fingerprint":"synthetic","choice":0}"#,
            Code(-32009),
        ),
        (
            "session.respondRunnerRequest",
            r#"{"sessionId":"worker-2222bbbb","fingerprint":"synthetic","response":{"decision":"decline"}}"#,
            Code(-32009),
        ),
        (
            "session.interrupt",
            r#"{"sessionId":"worker-2222bbbb"}"#,
            Code(-32009),
        ),
        ("decision.list", "null", Ok),
        ("wake.list", "null", Ok),
        ("loop.list", "null", Ok),
        (
            "loop.recover",
            r#"{"scope":"session","targetSessionId":"worker-2222bbbb"}"#,
            Ok,
        ),
        ("settings.get", "null", Ok),
        ("settings.set", r#"{"maxLiveSessions":null}"#, Ok),
        ("message.receipts", r#"{"messageId":"m1"}"#, Ok),
        // Hibernate ends the server, so it is the final method in this table.
        ("crewd.hibernate", "null", Ok),
    ];
    let mut covered: Vec<&str> = cases.iter().map(|c| c.0).collect();
    covered.sort();
    covered.dedup();
    let mut all = crewd_proto::methods::ALL.to_vec();
    all.sort();
    assert_eq!(covered, all, "加了新方法就在这里加一条");
    let mut created_work = Vec::new();
    let mut created_document = None;
    for (method, params, want) in cases {
        let out: Output = crewd(data.path())
            .args(["rpc", method, params])
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        match want {
            Ok => assert!(out.status.success(), "{method}: {err}"),
            Code(c) => {
                assert!(!out.status.success(), "{method} 应当失败");
                assert!(
                    err.contains(&format!("({c})")),
                    "{method}: 期望 {c}，实际 {err}"
                );
            }
        }
        if method == "work.create" {
            let first: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            created_work.push(first["id"].as_str().unwrap().to_owned());
            for (method, expected) in [
                ("work.reassignLost", -32602),
                ("work.transferRequest", -32003),
                ("work.transferAccept", -32009),
                ("work.transferWithdraw", -32009),
            ] {
                let mut args =
                    serde_json::json!({"id":first["id"],"expectedRevision":first["revision"]});
                if method == "work.reassignLost" {
                    args["owner"] = serde_json::json!("worker-2222bbbb");
                    args["note"] = serde_json::json!("No caller-declared loss");
                }
                if method == "work.transferRequest" {
                    args["to"] = serde_json::json!("worker-2222bbbb");
                    args["note"] = serde_json::json!("Human cannot impersonate an owner");
                }
                let refused = crewd(data.path())
                    .args(["rpc", method, &args.to_string()])
                    .output()
                    .unwrap();
                assert!(!refused.status.success(), "{method}");
                assert!(
                    String::from_utf8_lossy(&refused.stderr).contains(&format!("({expected})")),
                    "{method}: {:?}",
                    refused.stderr
                );
                assert_eq!(
                    rpc(
                        data.path(),
                        "work.get",
                        &serde_json::json!({"id":first["id"]}).to_string()
                    ),
                    first
                );
            }

            let second = rpc(
                data.path(),
                "work.create",
                r#"{"crewId":"local-aaaa","title":"second synthetic plan"}"#,
            );
            created_work.push(second["id"].as_str().unwrap().to_owned());
            assert_ne!(created_work[0], created_work[1]);
        } else if method == "document.create" {
            let first: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            let id = first["document"]["id"].as_str().unwrap();
            let original = rpc(
                data.path(),
                "document.get",
                &serde_json::json!({"id":id}).to_string(),
            );
            assert_eq!(original["body"], "CLI original 🙂");
            let edited=rpc(data.path(),"document.update",&serde_json::json!({"id":id,"expectedRevision":original["revision"],"title":"CLI edited","body":"changed 🐾"}).to_string());
            assert_eq!(edited["revision"], "2");
            let retry = rpc(data.path(), "document.create", params);
            assert_eq!(retry["reused"], true);
            assert_eq!(retry["document"], edited);
            created_document = Some(id.to_string());
        } else if method == "viewFolder.create" {
            let first: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            let folder = first["folders"][0]["id"].as_str().unwrap();
            let doc = created_document.as_ref().unwrap();
            let assigned=rpc(data.path(),"viewFolder.assign",&serde_json::json!({"crewId":"local-aaaa","view":"documents","expectedTreeRevision":first["treeRevision"],"object":{"kind":"document","id":doc},"folderId":folder}).to_string());
            assert_eq!(assigned["assignments"][0]["object"]["id"], doc.as_str());
            let updated=rpc(data.path(),"viewFolder.update",&serde_json::json!({"crewId":"local-aaaa","view":"documents","id":folder,"expectedTreeRevision":assigned["treeRevision"],"name":"renamed","position":0}).to_string());
            assert_eq!(updated["folders"][0]["name"], "renamed");
            let deleted=rpc(data.path(),"viewFolder.delete",&serde_json::json!({"crewId":"local-aaaa","view":"documents","id":folder,"expectedTreeRevision":updated["treeRevision"]}).to_string());
            assert_eq!(deleted["folders"], serde_json::json!([]));
            assert_eq!(deleted["assignments"], serde_json::json!([]));
            let d = rpc(
                data.path(),
                "document.get",
                &serde_json::json!({"id":doc}).to_string(),
            );
            assert_eq!(d["body"], "changed 🐾");
            let gone = rpc(
                data.path(),
                "document.delete",
                &serde_json::json!({"id":doc,"expectedRevision":d["revision"]}).to_string(),
            );
            assert!(gone["deletedAt"].is_string());
            assert_eq!(
                rpc(
                    data.path(),
                    "document.get",
                    &serde_json::json!({"id":doc}).to_string()
                )["body"],
                d["body"]
            );
        } else if method == "channel.workList" {
            let first: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(first["items"], serde_json::json!([]));
            let o = serde_json::json!({"kind":"work","id":created_work[0]});
            let linked=rpc(data.path(),"channel.workLink",&serde_json::json!({"channelId":"local-aaaa/lobby","expectedLinkRevision":first["linkRevision"],"object":o}).to_string());
            assert_eq!(linked["linked"], true);
            let view = rpc(
                data.path(),
                "channel.workList",
                r#"{"channelId":"local-aaaa/lobby"}"#,
            );
            assert_eq!(view["items"][0]["object"], o);
            let unlinked=rpc(data.path(),"channel.workUnlink",&serde_json::json!({"channelId":"local-aaaa/lobby","expectedLinkRevision":view["linkRevision"],"object":o}).to_string());
            assert_eq!(unlinked["linked"], false);
            assert!(
                rpc(
                    data.path(),
                    "work.get",
                    &serde_json::json!({"id":created_work[0]}).to_string()
                )["id"]
                    .is_string()
            );
        } else if method == "todo.create" {
            let first: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            let id = first["requirementId"].as_str().unwrap();
            assert!(id.starts_with('#'));
            assert_eq!(first["text"], "  CLI source\n原文不变  ");
            assert_eq!(first["status"], "unclaimed");
            let detail = rpc(
                data.path(),
                "todo.get",
                &serde_json::json!({"id":id}).to_string(),
            );
            assert_eq!(detail, first);
            assert_eq!(rpc(data.path(), "todo.create", params), first);
            let mut changed: serde_json::Value = serde_json::from_str(params).unwrap();
            changed["text"] = serde_json::json!("changed original must conflict");
            let conflict = crewd(data.path())
                .args(["rpc", "todo.create", &changed.to_string()])
                .output()
                .unwrap();
            assert!(!conflict.status.success());
            assert!(String::from_utf8_lossy(&conflict.stderr).contains("(-32009)"));
            let closed=rpc(data.path(),"todo.close",&serde_json::json!({"id":id,"expectedStatus":"unclaimed","note":"explicit CLI human ruling"}).to_string());
            assert_eq!(closed["status"], "closed");
            assert_eq!(closed["text"], first["text"]);
            assert_eq!(closed["closure"]["note"], "explicit CLI human ruling");
            assert!(closed["closure"]["at"].is_string());
            assert_eq!(
                rpc(
                    data.path(),
                    "todo.get",
                    &serde_json::json!({"id":id}).to_string()
                ),
                closed
            );
            assert_eq!(
                rpc(data.path(), "todo.create", params),
                closed,
                "same request returns the latest derived view"
            );
        } else if method == "session.usage" {
            let evidence: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(
                evidence["items"],
                serde_json::json!([]),
                "no model notification must remain unknown, not fabricated zeros"
            );
            assert!(evidence["nextAfter"].is_null());
        } else if method == "work.patrol" {
            let first: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(first["items"].as_array().unwrap().len(), 1);
            assert_eq!(first["items"][0]["id"], created_work[0]);
            assert_eq!(first["items"][0]["crewId"], "local-aaaa");
            assert_eq!(first["items"][0]["unowned"], true);
            assert_eq!(first["items"][0]["stopEscalated"], false);
            assert_eq!(first["items"][0]["longWaits"], serde_json::json!([]));
            assert!(first["items"][0]["revision"].is_string());
            assert!(first["observedAt"].is_string());
            assert_eq!(first["liveOnly"], true);
            assert_eq!(first["includeRemoved"], false);
            assert_eq!(first["nextAfter"], created_work[0]);
            let second = rpc(
                data.path(),
                "work.patrol",
                &serde_json::json!({
                    "crewId":"local-aaaa", "limit":1, "after":first["nextAfter"]
                })
                .to_string(),
            );
            assert_eq!(second["items"].as_array().unwrap().len(), 1);
            assert_eq!(second["items"][0]["id"], created_work[1]);
            assert_eq!(second["items"][0]["unowned"], true);
            assert!(second["nextAfter"].is_null());
            let detail = rpc(
                data.path(),
                "work.get",
                &serde_json::json!({"id":second["items"][0]["id"]}).to_string(),
            );
            assert_eq!(detail["id"], created_work[1]);
            assert_eq!(detail["title"], "second synthetic plan");
            let exhausted = rpc(
                data.path(),
                "work.patrol",
                &serde_json::json!({
                    "crewId":"local-aaaa", "limit":1, "after":second["items"][0]["id"]
                })
                .to_string(),
            );
            assert_eq!(exhausted["items"], serde_json::json!([]));
            assert!(exhausted["nextAfter"].is_null());
        }
    }
}
