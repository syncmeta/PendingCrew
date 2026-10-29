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
    let remote_status = rpc(data.path(), "remote.status", "null");
    let remote_configuration = serde_json::json!({
        "requestId":"11111111-1111-4111-8111-111111111111",
        "expectedRevision":remote_status["configuration"]["revision"],
        "enabled":false,"listenHost":"127.0.0.1","port":7423
    })
    .to_string();
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
        // Legacy document CRUD is withdrawn in favor of SQLite knowledge.
        // These typed negatives cover routing, not legacy document usability.
        (
            "document.create",
            r#"{"crewId":"local-aaaa","title":"missing create request"}"#,
            Code(-32602),
        ),
        (
            "document.get",
            r#"{"id":7}"#,
            Code(-32602),
        ),
        ("document.list", r#"{"crewId":7}"#, Code(-32602)),
        (
            "document.update",
            r#"{"id":"11111111-1111-4111-8111-111111111111","title":"missing revision"}"#,
            Code(-32602),
        ),
        (
            "document.delete",
            r#"{"id":"11111111-1111-4111-8111-111111111111"}"#,
            Code(-32602),
        ),
        (
            "viewFolder.list",
            r#"{"crewId":"local-aaaa","view":"progress"}"#,
            Ok,
        ),
        (
            "viewFolder.create",
            r#"{"requestId":"56fc7f71-3fde-494b-a163-7c1a646af4af","crewId":"local-aaaa","view":"progress","expectedTreeRevision":"0","name":"CLI folder"}"#,
            Ok,
        ),
        // The preceding progress-folder cycle advanced this tree revision.
        // Stale revision 0 must conflict before object lookup or mutation.
        (
            "viewFolder.update",
            r#"{"crewId":"local-aaaa","view":"progress","id":"11111111-1111-4111-8111-111111111111","expectedTreeRevision":"0","name":"x","position":0}"#,
            Code(-32009),
        ),
        (
            "viewFolder.delete",
            r#"{"crewId":"local-aaaa","view":"progress","id":"11111111-1111-4111-8111-111111111111","expectedTreeRevision":"0"}"#,
            Code(-32009),
        ),
        (
            "viewFolder.assign",
            r#"{"crewId":"local-aaaa","view":"progress","expectedTreeRevision":"0","object":{"kind":"work","id":"W999"}}"#,
            Code(-32009),
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
        // Remote setup remains disabled: these are real public CLI handlers,
        // with explicit negative controls for no listener/invalid reply/no grant.
        ("remote.status", "null", Ok),
        ("remote.grants", "null", Ok),
        ("remote.configure", remote_configuration.as_str(), Ok),
        (
            "remote.invite",
            r#"{"requestId":"22222222-2222-4222-8222-222222222222"}"#,
            Code(-32602),
        ),
        (
            "remote.accept",
            r#"{"requestId":"33333333-3333-4333-8333-333333333333","responseEnvelope":{"format":1,"payload":"","signature":""}}"#,
            Code(-32602),
        ),
        (
            "remote.revoke",
            r#"{"grantId":"44444444-4444-4444-8444-444444444444","expectedRevision":"1","note":"No grant has been accepted"}"#,
            Code(-32009),
        ),
        // Host metadata is read through actual CLI RPCs. Codex is disabled in
        // this isolated fixture, so quota must refuse without a provider launch.
        ("host.quota", "null", Code(-32009)),
        ("host.quota", r#"{"unexpected":true}"#, Code(-32602)),
        ("host.directoryList", "{}", Ok),
        ("host.directoryList", r#"{"path":"relative"}"#, Code(-32602)),
        // These closed public methods are advertised on both supported hosts.
        // Null must reach their typed parser and fail before any durable effect;
        // production payload/lifecycle coverage lives in the public physical gate.
        ("executor.list", "null", Code(-32602)),
        ("command.submit", "null", Code(-32602)),
        ("command.lookup", "null", Code(-32602)),
        ("command.listUnknown", "null", Code(-32602)),
        ("session.controlTarget", "null", Code(-32602)),
        ("session.terminal.inspect", "null", Code(-32602)),
        ("session.terminal.watch", "null", Code(-32602)),
        // Optional judgment configuration is a real local Human route. Typed
        // invalid writes never read a credential or contact a provider.
        ("judgment.settings.get", "null", Ok),
        ("judgment.settings.set", "null", Code(-32602)),
        ("judgment.inspect", "null", Code(-32602)),
        // Knowledge writes require a request/revision/source envelope; a channel
        // alone must be rejected before a link or workspace can be created.
        ("channel.knowledgeLink", r#"{"channelId":"local-aaaa/lobby"}"#, Code(-32602)),
        ("channel.knowledgeUnlink", r#"{"channelId":"local-aaaa/lobby"}"#, Code(-32602)),
        ("channel.knowledgeList", r#"{"channelId":"local-aaaa/lobby","limit":-1}"#, Code(-32602)),
        ("channel.workTagsSet", r#"{"channelId":"local-aaaa/lobby","tags":[]}"#, Code(-32602)),
        // A crew ID is not a bound DataContext. Each route must parse its own
        // closed DTO and reject the missing authority/DB/epoch/generation before
        // any workspace initialization, SQL submission, preview or restore.
        ("data.confirm", r#"{"context":{"crewId":"local-aaaa"}}"#, Code(-32602)),
        ("data.history", r#"{"context":{"crewId":"local-aaaa"}}"#, Code(-32602)),
        ("data.object.get", r#"{"context":{"crewId":"local-aaaa"}}"#, Code(-32602)),
        ("data.object.list", r#"{"context":{"crewId":"local-aaaa"}}"#, Code(-32602)),
        ("data.preview", r#"{"context":{"crewId":"local-aaaa"},"statements":[]}"#, Code(-32602)),
        ("data.preview.get", r#"{"context":{"crewId":"local-aaaa"}}"#, Code(-32602)),
        ("data.query", r#"{"context":{"crewId":"local-aaaa"}}"#, Code(-32602)),
        ("data.request.get", r#"{"context":{"crewId":"local-aaaa"}}"#, Code(-32602)),
        ("data.restore", r#"{"context":{"crewId":"local-aaaa"}}"#, Code(-32602)),
        ("data.schema.get", r#"{"context":{"crewId":"local-aaaa"}}"#, Code(-32602)),
        ("data.snapshots", r#"{"context":{"crewId":"local-aaaa"}}"#, Code(-32602)),
        ("data.submit", r#"{"context":{"crewId":"local-aaaa"}}"#, Code(-32602)),
        ("data.table.get", r#"{"context":{"crewId":"local-aaaa"}}"#, Code(-32602)),
        // Workspace inspection itself is a safe read, including uninitialized.
        ("data.workspace.get", r#"{"crewId":"local-aaaa"}"#, Ok),
        // Missing Human task text/request is not permission to create an ask.
        ("humanTask.create", r#"{"crewId":"local-aaaa","channelId":"local-aaaa/lobby"}"#, Code(-32602)),
        // Each rpc CLI invocation is an independent observer connection. Do not
        // claim management or exit the fixture: malformed empty-DTO objects are
        // rejected; an actual exit by a non-owner is forbidden.
        ("lifecycle.status", "{}", Ok),
        ("lifecycle.claim", r#"{"unexpected":true}"#, Code(-32602)),
        ("lifecycle.release", r#"{"unexpected":true}"#, Code(-32602)),
        ("lifecycle.exit", r#"{"mode":"safePause"}"#, Code(-32003)),
        ("local.clientTrust.status", "{}", Ok),
        ("personalDraft.status", "{}", Ok),
        // A partial space/member scope cannot authorize personal draft access.
        ("personalDraft.branches", r#"{"scope":{"spaceId":"not-approved"}}"#, Code(-32602)),
        ("personalDraft.catalog", r#"{"scope":{"spaceId":"not-approved"}}"#, Code(-32602)),
        ("personalDraft.members", r#"{"spaceId":"not-approved"}"#, Code(-32602)),
        ("personalDraft.requestFence", r#"{"original":{}}"#, Code(-32602)),
        // No token is generated or enrollment begun: the signed/UUID envelopes
        // remain required even while the fixture listener is disabled.
        ("remote.enrollmentCreate", r#"{"computerLabel":"offline fixture"}"#, Code(-32602)),
        ("remote.enrollmentLookup", r#"{"enrollmentId":"11111111-1111-4111-8111-111111111111"}"#, Code(-32602)),
        ("resource.requestLookup", r#"{"requestId":7}"#, Code(-32602)),
        // These DTO negatives exercise the public typed parsers, not provider
        // history/model execution. No valid input bytes or live source is given.
        ("session.claude.history", r#"{"sessionId":7}"#, Code(-32602)),
        ("session.claude.inspect", r#"{"sessionId":7}"#, Code(-32602)),
        ("session.claude.recoveryInspect", r#"{"sessionId":7}"#, Code(-32602)),
        // The imported worker is Codex: a typed existing-session recovery read
        // must refuse Claude recovery evidence, rather than invent eligibility.
        ("session.claude.recoveryInspect", r#"{"sessionId":"worker-2222bbbb"}"#, Code(-32009)),
        ("session.claude.taskGet", r#"{"sessionId":"worker-2222bbbb"}"#, Code(-32602)),
        ("session.codex.history", r#"{"sessionId":7}"#, Code(-32602)),
        ("session.codex.inspect", r#"{"sessionId":7}"#, Code(-32602)),
        ("session.codex.models", r#"{"sessionId":7}"#, Code(-32602)),
        ("session.codex.permissionProfiles", r#"{"sessionId":7}"#, Code(-32602)),
        ("session.codex.tools", r#"{"sessionId":7}"#, Code(-32602)),
        ("session.codex.inputDiscard", r#"{"crewId":"local-aaaa"}"#, Code(-32602)),
        ("session.codex.inputLookup", r#"{"crewId":"local-aaaa"}"#, Code(-32602)),
        ("session.codex.inputPrepare", r#"{"crewId":"local-aaaa","filename":"not-uploaded.txt"}"#, Code(-32602)),
        // Existing object identities remain typed, not arbitrary todo strings.
        ("todo.assign", r#"{"channelId":"local-aaaa/lobby","object":{"kind":"invalid"}}"#, Code(-32602)),
        ("todo.attributesGet", r#"{"object":{"kind":"invalid"}}"#, Code(-32602)),
        ("todo.setState", r#"{"channelId":"local-aaaa/lobby","object":{"kind":"invalid"}}"#, Code(-32602)),
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
        if method == "host.directoryList" && out.status.success() {
            let listing: crewd_proto::host::DirectoryResult =
                serde_json::from_slice(&out.stdout).expect("typed host directory response");
            assert_eq!(Path::new(&listing.path), TEST_HOME.path().canonicalize().unwrap());
            assert!(listing.entries.iter().all(|entry| Path::new(&entry.path).is_dir()));
            assert!(listing.next_after.is_none(), "synthetic HOME fits one page");
        }
        if method == "session.claude.recoveryInspect" && params.contains("worker-2222bbbb") {
            assert!(err.contains("original Claude recovery evidence changed or unavailable"), "{err}");
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
        } else if method == "viewFolder.create" {
            let first: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            let folder = first["folders"][0]["id"].as_str().unwrap();
            let work = &created_work[0];
            let original = rpc(data.path(), "work.get", &serde_json::json!({"id":work}).to_string());
            let assigned=rpc(data.path(),"viewFolder.assign",&serde_json::json!({"crewId":"local-aaaa","view":"progress","expectedTreeRevision":first["treeRevision"],"object":{"kind":"work","id":work},"folderId":folder}).to_string());
            assert_eq!(assigned["assignments"][0]["object"]["id"], work.as_str());
            let updated=rpc(data.path(),"viewFolder.update",&serde_json::json!({"crewId":"local-aaaa","view":"progress","id":folder,"expectedTreeRevision":assigned["treeRevision"],"name":"renamed","position":0}).to_string());
            assert_eq!(updated["folders"][0]["name"], "renamed");
            let deleted=rpc(data.path(),"viewFolder.delete",&serde_json::json!({"crewId":"local-aaaa","view":"progress","id":folder,"expectedTreeRevision":updated["treeRevision"]}).to_string());
            assert_eq!(deleted["folders"], serde_json::json!([]));
            assert_eq!(deleted["assignments"], serde_json::json!([]));
            assert_eq!(rpc(data.path(), "work.get", &serde_json::json!({"id":work}).to_string()), original);
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

/// CLI selection reaches the original profile, owned-history eligibility and
/// terminal capability together. The synthetic executable must never be invoked.
#[test]
fn explicit_claude_disable_and_existing_selection_contract() {
    use std::os::unix::fs::PermissionsExt;
    for mode in ["disabled", "discover", "explicit"] {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let work = root.path().join("work");
        let tools = root.path().join("tools");
        std::fs::create_dir(&data).unwrap();
        std::fs::create_dir(&work).unwrap();
        std::fs::create_dir(&tools).unwrap();
        let invoked = root.path().join("provider-was-invoked");
        let fake = tools.join("claude");
        std::fs::write(&fake, format!("#!/bin/sh\ntouch '{}'\nexit 90\n", invoked.display())).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
        {
            let store = crewd_store::Store::open_data_dir(&data).unwrap();
            crewd_store::crews::upsert(store.conn(), &crewd_proto::Crew {
                id: "disabled-crew".into(), title: "Synthetic configuration".into(),
                working_directory: Some(work.display().to_string()),
                created_at: "2026-10-25T00:00:00.000Z".into(), updated_at: "2026-10-25T00:00:00.000Z".into(),
                ..Default::default()
            }, None).unwrap();
            crewd_store::sessions::upsert(store.conn(), &crewd_proto::Session {
                id: "disabled-session".into(), crew_id: "disabled-crew".into(), role: crewd_proto::Role::Worker,
                executor: "claude".into(), agent_session_id: Some("00000000-0000-4000-8000-000000000001".into()),
                working_directory: Some(work.display().to_string()), desired: crewd_proto::Desired::Stopped,
                updated_at: "2026-10-25T00:00:00.000Z".into(), ..Default::default()
            }, None).unwrap();
        }
        let mut command = crewd(&data);
        command.env_remove("CREWD_CLAUDE").env("PATH", format!("{}:/usr/bin:/bin", tools.display())).arg("serve");
        match mode {
            "disabled" => { command.arg("--disable-claude"); }
            "explicit" => { command.arg("--claude").arg(&fake); }
            _ => {}
        }
        let mut daemon = Daemon(command.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(10);
        while !data.join("crewd.sock").exists() {
            if let Some(status) = daemon.0.try_wait().unwrap() { panic!("{mode} serve exited before readiness: {status}"); }
            assert!(Instant::now() < deadline, "{mode} socket readiness timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
        let expected = mode != "disabled";
        let listed = rpc(&data, "executor.list", r#"{"crewId":"disabled-crew"}"#);
        let profiles = listed["profiles"].as_array().unwrap();
        let claude = profiles.iter().find(|p| p["id"] == "claude").unwrap();
        assert_eq!(claude["available"], expected, "{mode}: {listed}");
        if !expected { assert_eq!(claude["unavailableReason"], "notConfigured"); }
        assert_eq!(profiles.iter().find(|p| p["id"] == "codex").unwrap()["available"], false);
        let inspection = rpc(&data, "session.claude.inspect", r#"{"sessionId":"disabled-session"}"#);
        assert_eq!(inspection["manualResume"]["eligibility"], if expected {"eligible"} else {"notConfigured"}, "{mode}: {inspection}");
        assert_eq!(inspection["manualResume"]["originalAgentSessionId"], "00000000-0000-4000-8000-000000000001");
        let hello = rpc(&data, "system.hello", "null");
        assert_eq!(hello["capabilities"].as_array().unwrap().iter().any(|v| v == "session.terminal"), expected, "{mode}: {hello}");
        assert!(!invoked.exists(), "{mode} ran a provider during configuration reads");
        // Only this fixture's owned daemon; no production process is signaled.
        assert_eq!(unsafe { libc::kill(daemon.0.id() as i32, libc::SIGTERM) }, 0);
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            if let Some(status) = daemon.0.try_wait().unwrap() { assert!(status.success(), "{mode}: {status}"); break; }
            assert!(Instant::now() < deadline, "{mode} normal shutdown timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn explicit_claude_disable_conflicts_with_path_and_environment_before_storage() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("must-not-exist");
    for environment in [false, true] {
        let mut command = crewd(&data);
        command.env_remove("CREWD_CLAUDE").args(["serve", "--disable-claude"]);
        if environment { command.env("CREWD_CLAUDE", "/usr/bin/false"); }
        else { command.args(["--claude", "/usr/bin/false"]); }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("cannot be used with"), "must reject an option conflict, not an unknown flag: {error}");
        assert!(!data.exists(), "argument rejection created storage");
    }
}
