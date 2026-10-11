//! Opt-in synthetic managed peer. Default fake_claude behavior is untouched.
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::{Duration, Instant},
};
fn await_file(p: &Path) -> std::io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !p.is_file() {
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "synthetic peer command missing",
            ));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}
fn exchange(socket: &Path, requests: &[Value]) -> std::io::Result<Vec<Value>> {
    let mut out = Vec::new();
    let stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    for r in requests {
        writeln!(writer, "{r}")?;
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                return Err(std::io::Error::other("EOF before response"));
            }
            let value: Value = serde_json::from_str(&line)?;
            if value["id"] == r["id"] {
                out.push(value);
                break;
            }
        }
    }
    Ok(out)
}
pub fn run(dir: &Path, child: bool, args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let tag = if child { "child" } else { "root" };
    std::fs::write(
        dir.join(format!("{tag}.ready")),
        std::process::id().to_string(),
    )?;
    await_file(&dir.join("go"))?; // Root test verifies genuine driver registration before issuing command.
    let spec: Value = serde_json::from_slice(&std::fs::read(dir.join("requests.json"))?)?;
    let socket = Path::new(spec["socket"].as_str().ok_or("socket")?);
    if !child && spec["scenario"].as_str().is_some() {
        return run_reparent_root(dir, &spec);
    }
    let post =
        json!({"channelId":spec["channel"],"text":format!("S1_SYNTHETIC_{tag}"),"mentions":[]});
    let mut req = vec![
        json!({"id":1,"method":"system.hello","params":{"client":{"name":"human","version":"test"}}}),
        json!({"id":2,"method":"message.post","params":post}),
        json!({"id":3,"method":"events.subscribe","params":{"topics":["crew"]}}),
    ];
    if !child {
        let cfg = args
            .iter()
            .position(|v| v == "--mcp-config")
            .and_then(|i| args.get(i + 1))
            .ok_or("generated mcp config missing")?;
        let mcp: Value = serde_json::from_slice(&std::fs::read(cfg)?)?;
        let positive = generated_helper_positive(&mcp["mcpServers"]["crew"])?;
        std::fs::write(
            dir.join("helper.positive.json"),
            serde_json::to_vec(&positive)?,
        )?;
        let helper_args = mcp["mcpServers"]["crew"]["args"]
            .as_array()
            .ok_or("generated helper args missing")?;
        let read_arg = |flag: &str| -> Result<String, Box<dyn std::error::Error>> {
            let i = helper_args
                .iter()
                .position(|v| v.as_str() == Some(flag))
                .ok_or("helper flag missing")?;
            Ok(helper_args
                .get(i + 1)
                .and_then(Value::as_str)
                .ok_or("helper value missing")?
                .into())
        };
        let token = std::fs::read_to_string(read_arg("--token-file")?)?;
        req.push(json!({"id":4,"method":"agent.attach","params":{"sessionId":read_arg("--session")?,"token":token.trim()}}));
        req.push(json!({"id":5,"method":"agent.tools","params":{}}));
        req.push(json!({"id":6,"method":"message.post","params":{"channelId":spec["channel"],"text":"S1_AFTER_ATTACH","mentions":[]}}));
    }
    let responses = exchange(socket, &req)?;
    std::fs::write(
        dir.join(format!("{tag}.responses.json")),
        serde_json::to_vec(&responses)?,
    )?;
    if !child {
        let mut helper = std::process::Command::new(std::env::current_exe()?)
            .arg("--s1-peer-child")
            .arg(dir)
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(15);
        let (normal, code) = loop {
            if let Some(s) = helper.try_wait()? {
                break (true, s.code());
            }
            if Instant::now() >= deadline {
                helper.kill()?;
                let s = helper.wait()?;
                break (false, s.code());
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        std::fs::write(
            dir.join("child.exit.json"),
            serde_json::to_vec(&json!({"normal":normal,"code":code}))?,
        )?;
        // Remain owned by the original real driver until the parent closes its normal session.
        await_file(&dir.join("finish"))?;
    }
    Ok(())
}
fn generated_helper_positive(crew: &Value) -> Result<Value, Box<dyn std::error::Error>> {
    use std::process::Stdio;
    let program = crew["command"]
        .as_str()
        .ok_or("generated helper command missing")?;
    let args = crew["args"]
        .as_array()
        .ok_or("generated helper args missing")?;
    let mut child = std::process::Command::new(program)
        .args(args.iter().map(|v| v.as_str().unwrap_or("")))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let mut input = child.stdin.take().ok_or("helper stdin")?;
    let output = child.stdout.take().ok_or("helper stdout")?;
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            match line {
                Ok(v) => {
                    if tx.send(v).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let observed = (|| -> Result<Vec<Value>, Box<dyn std::error::Error>> {
        let mut responses = Vec::new();
        for (id, method, params) in [
            (
                1,
                "initialize",
                json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"synthetic-s1","version":"0"}}),
            ),
            (2, "tools/list", json!({})),
        ] {
            writeln!(
                input,
                "{}",
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
            )?;
            let until = Instant::now() + Duration::from_secs(5);
            loop {
                let line = rx.recv_timeout(until.saturating_duration_since(Instant::now()))?;
                let frame: Value = serde_json::from_str(&line)?;
                if frame["id"] == id {
                    responses.push(frame);
                    break;
                }
            }
            if id == 1 {
                writeln!(
                    input,
                    "{}",
                    json!({"jsonrpc":"2.0","method":"notifications/initialized"})
                )?;
            }
        }
        Ok(responses)
    })();
    drop(input);
    let until = Instant::now() + Duration::from_secs(5);
    let (normal, code) = loop {
        if let Some(s) = child.try_wait()? {
            break (true, s.code());
        }
        if Instant::now() >= until {
            child.kill()?;
            let s = child.wait()?;
            break (false, s.code());
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let _ = reader.join();
    Ok(json!({"responses":observed?,"normal":normal,"code":code}))
}

// Explicit second fixture; default root/grandchild and real generated MCP remain unchanged.
fn kernel_identity(
    pid: i32,
) -> Result<crewd_exec::registry::ProcessIdentity, Box<dyn std::error::Error>> {
    match crewd_exec::registry::probe(pid) {
        crewd_exec::registry::ProbeResult::Found(identity) => Ok(identity),
        _ => Err("cannot observe synthetic kernel birth".into()),
    }
}
fn same_process(
    a: &crewd_exec::registry::ProcessIdentity,
    b: &crewd_exec::registry::ProcessIdentity,
) -> bool {
    a.pid == b.pid && a.start_time == b.start_time && a.boot_id == b.boot_id
}
fn original_gone(id: &crewd_exec::registry::ProcessIdentity) -> bool {
    match crewd_exec::registry::probe(id.pid) {
        crewd_exec::registry::ProbeResult::Missing => true,
        crewd_exec::registry::ProbeResult::Found(current) => !same_process(id, &current),
        crewd_exec::registry::ProbeResult::Unreadable(_) => false,
    }
}
fn bounded_condition(
    dir: &Path,
    mut yes: impl FnMut() -> bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        if yes() {
            return Ok(());
        }
        if dir.join("finish").is_file() {
            return Err("synthetic fixture cancelled".into());
        }
        if Instant::now() >= until {
            return Err("synthetic kernel/command barrier timed out".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn owned_wait(child: &mut std::process::Child) -> Result<Value, Box<dyn std::error::Error>> {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(json!({"normal":true,"code":status.code()}));
        }
        if Instant::now() >= until {
            child.kill()?;
            let status = child.wait()?;
            return Ok(json!({"normal":false,"code":status.code()}));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn run_reparent_root(dir: &Path, spec: &Value) -> Result<(), Box<dyn std::error::Error>> {
    let mut intermediate = std::process::Command::new(std::env::current_exe()?)
        .arg("--s1-peer-intermediate")
        .arg(dir)
        .spawn()?;
    let receipt = owned_wait(&mut intermediate)?;
    std::fs::write(
        dir.join("intermediate.exit.json"),
        serde_json::to_vec(&receipt)?,
    )?;
    if spec["scenario"] == "root-exit" {
        // Normal return leaves the original registered driver root. The orphan proves
        // that original birth is actually gone, rather than trusting this file as Exit.
        std::fs::write(
            dir.join("root.normal-exit.intent"),
            b"returning from fixture",
        )?;
        return Ok(());
    }
    bounded_condition(dir, || dir.join("orphan.terminal.intent.json").is_file())?;
    await_file(&dir.join("finish"))?;
    Ok(())
}
// Direct-child guard exists immediately after spawn, before any fallible probe/file write.
// Only the successful intentional-detach path disarms it. No group signal is used.
struct OwnedOrphanBeforeDetach(Option<std::process::Child>);
impl Drop for OwnedOrphanBeforeDetach {
    fn drop(&mut self) {
        let Some(mut child) = self.0.take() else {
            return;
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                eprintln!("S1 pre-detach child already waited: {status}");
                return;
            }
            Err(error) => eprintln!("S1 pre-detach try_wait failed: {error}"),
            Ok(None) => {}
        }
        let killed = child.kill();
        eprintln!("S1 pre-detach exact direct-child kill: {killed:?}");
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    eprintln!("S1 pre-detach exact direct-child waited: {status}");
                    return;
                }
                Err(error) => {
                    eprintln!("S1 pre-detach reap failed: {error}");
                    return;
                }
                Ok(None) => {}
            }
            if Instant::now() >= deadline {
                eprintln!("S1 pre-detach child cleanup timed out; no normal reap claimed");
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
pub fn run_intermediate(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    // This synthetic orphan must outlive the real registered root for the test.
    // It is still a genuine descendant when the first socket identity is captured.
    // Close inherited PTY slave descriptors; setsid alone does not close them.
    let stderr = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(dir.join("orphan.stderr.log"))?;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .arg("--s1-peer-orphan")
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr));
    // SAFETY: pre_exec performs only the async-signal-safe setsid syscall and errno read.
    // No signal-ignore, authority flag, registry change, allocation or PID guess.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut orphan = OwnedOrphanBeforeDetach(Some(command.spawn()?));
    let pid = orphan.0.as_ref().expect("owned before detach").id() as i32;
    std::fs::write(
        dir.join("orphan.spawn.json"),
        serde_json::to_vec(&kernel_identity(pid)?)?,
    )?;
    bounded_condition(dir, || dir.join("orphan.before.json").is_file())?;
    // Sole intentional detach point: all previous Err/unwind paths retain guard cleanup.
    // No orphan wait status is invented when relinquishing direct-child ownership.
    drop(orphan.0.take());
    Ok(())
}
fn exchange_one(
    writer: &mut UnixStream,
    reader: &mut BufReader<UnixStream>,
    request: Value,
) -> std::io::Result<Value> {
    writeln!(writer, "{request}")?;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(std::io::Error::other("EOF before fixture reply"));
        }
        let response: Value = serde_json::from_str(&line)?;
        if response["id"] == request["id"] {
            return Ok(response);
        }
    }
}
fn orphan_phase(dir: &Path, phase: &str, error: Option<&str>) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(dir.join("orphan.phases.jsonl"))?;
    // Only synthetic lifecycle/kernel metadata. Never dump request/token/response bodies.
    let metadata = json!({"phase":phase,"pid":std::process::id(),
        "parent":unsafe { libc::getppid() }, "pgid":unsafe { libc::getpgrp() },
        "sid":unsafe { libc::getsid(0) },
        "error":error.map(|s| s.chars().take(512).collect::<String>())});
    writeln!(log, "{metadata}")
}
pub fn run_orphan(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    orphan_phase(dir, "entered-independent-session", None)?;
    let result = run_orphan_inner(dir);
    match &result {
        Ok(()) => orphan_phase(dir, "normal-return-after-terminal-intent", None)?,
        Err(error) => {
            let _ = orphan_phase(dir, "error-return", Some(&error.to_string()));
        }
    }
    result
}
fn run_orphan_inner(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let spec: Value = serde_json::from_slice(&std::fs::read(dir.join("requests.json"))?)?;
    let socket = Path::new(spec["socket"].as_str().ok_or("socket")?);
    let pid = std::process::id() as i32;
    let birth = kernel_identity(pid)?;
    // SAFETY: getppid reads only this disposable process's kernel parent identity.
    let parent_before = unsafe { libc::getppid() };
    let stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut before = Vec::new();
    for (id, method, params) in [
        (
            1,
            "system.hello",
            json!({"client":{"name":"human","version":"test"}}),
        ),
        (
            2,
            "message.post",
            json!({"channelId":spec["channel"],"text":"S1_ORPHAN_BEFORE","mentions":[]}),
        ),
        (3, "events.subscribe", json!({"topics":["message"]})),
    ] {
        before.push(exchange_one(
            &mut writer,
            &mut reader,
            json!({"id":id,"method":method,"params":params}),
        )?);
    }
    std::fs::write(dir.join("orphan.before.json"), serde_json::to_vec(&before)?)?;
    orphan_phase(dir, "before-responses-complete", None)?;
    bounded_condition(dir, || unsafe { libc::getppid() } != parent_before)?;
    let parent_after = unsafe { libc::getppid() };
    orphan_phase(dir, "actual-parent-changed", None)?;
    let root_pid = std::fs::read_to_string(dir.join("root.ready"))?.parse::<i32>()?;
    if spec["scenario"] == "root-exit" {
        let original: crewd_exec::registry::ProcessIdentity =
            serde_json::from_slice(&std::fs::read(dir.join("actual.root.birth.json"))?)?;
        if original.pid != root_pid {
            return Err("original root mismatch".into());
        }
        orphan_phase(dir, "waiting-original-root-birth-gone", None)?;
        bounded_condition(dir, || original_gone(&original))?;
        orphan_phase(dir, "original-root-birth-gone", None)?;
    }
    let birth_after = kernel_identity(pid)?;
    let mut after = Vec::new();
    for (id, method, params) in [
        (
            4,
            "system.hello",
            json!({"client":{"name":"human","version":"test"}}),
        ),
        (
            5,
            "message.post",
            json!({"channelId":spec["channel"],"text":"S1_ORPHAN_AFTER","mentions":[]}),
        ),
        (6, "events.subscribe", json!({"topics":["message"]})),
    ] {
        after.push(exchange_one(
            &mut writer,
            &mut reader,
            json!({"id":id,"method":method,"params":params}),
        )?);
    }
    let reconnect = exchange(
        socket,
        &[
            json!({"id":7,"method":"system.hello","params":{"client":{"name":"human","version":"test"}}}),
            json!({"id":8,"method":"message.post","params":{"channelId":spec["channel"],"text":"S1_ORPHAN_RECONNECTED","mentions":[]}}),
            json!({"id":9,"method":"events.subscribe","params":{"topics":["message"]}}),
        ],
    )?;
    std::fs::write(
        dir.join("orphan.after.json"),
        serde_json::to_vec(&json!({
        "responses":after,"reconnect":reconnect,"birth_before":birth,"birth_after":birth_after,
        "parent_before":parent_before,"parent_after":parent_after,"root_pid":root_pid}))?,
    )?;
    orphan_phase(dir, "after-and-reconnect-responses-complete", None)?;
    // An intent+actual birth disappearance is observable for a reparented process;
    // nobody falsely reports a wait/reap exit code for a non-child.
    std::fs::write(
        dir.join("orphan.terminal.intent.json"),
        serde_json::to_vec(&json!({"return_code_intent":0,"wait_status_available":false}))?,
    )?;
    Ok(())
}
