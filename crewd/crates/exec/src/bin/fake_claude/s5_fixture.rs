//! Opt-in compiled fake fixture; no default fake/provider behavior is changed.
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::{Duration, Instant},
};
fn publish(dir: &Path, name: &str, value: &Value) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = dir.join(format!(".{name}.complete-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    drop(file);
    std::fs::rename(tmp, dir.join(name))
}
fn gate(dir: &Path, name: &str, overall: Instant) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = (Instant::now() + Duration::from_secs(15)).min(overall);
    while !dir.join(name).is_file() {
        if dir.join("finish").is_file() {
            return Err("owned fixture cancelled".into());
        }
        if Instant::now() >= deadline {
            return Err("owned fixture phase timed out".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}
struct Wire {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
    events: Vec<Value>,
    overall: Instant,
}
impl Wire {
    fn frame(&mut self, deadline: Instant) -> Result<Value, Box<dyn std::error::Error>> {
        let remaining = deadline
            .min(self.overall)
            .saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("actual response deadline".into());
        }
        self.reader.get_ref().set_read_timeout(Some(remaining))?;
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Err("actual socket EOF".into());
        }
        Ok(serde_json::from_str(&line)?)
    }
    fn call(
        &mut self,
        id: u64,
        method: &str,
        params: Value,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let deadline = (Instant::now() + Duration::from_secs(5)).min(self.overall);
        self.writer
            .set_write_timeout(Some(deadline.saturating_duration_since(Instant::now())))?;
        writeln!(
            self.writer,
            "{}",
            json!({"id":id,"method":method,"params":params})
        )?;
        loop {
            let value = self.frame(deadline)?;
            if value.get("event").is_some() {
                self.events.push(value);
            } else if value["id"] == id {
                return Ok(value);
            } else {
                return Err("unexpected actual request ID".into());
            }
        }
    }
    fn finish_socket(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.writer.shutdown(std::net::Shutdown::Write)?;
        let deadline = (Instant::now() + Duration::from_secs(5)).min(self.overall);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("actual socket EOF deadline".into());
            }
            self.reader.get_ref().set_read_timeout(Some(remaining))?;
            let mut line = String::new();
            if self.reader.read_line(&mut line)? == 0 {
                return Ok(());
            }
            let value: Value = serde_json::from_str(&line)?;
            if value.get("event").is_none() {
                return Err("unexpected response in actual EOF tail".into());
            }
            self.events.push(value);
        }
    }
    fn role_event(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let deadline = (Instant::now() + Duration::from_secs(5)).min(self.overall);
        while !self
            .events
            .iter()
            .any(|v| v["event"] == "agent.tools_changed" && v["data"]["role"] == "captain")
        {
            let value = self.frame(deadline)?;
            if value.get("event").is_none() {
                return Err("unsolicited response during watcher receipt".into());
            }
            self.events.push(value);
        }
        Ok(())
    }
}
pub fn run(dir: &Path, args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let overall = Instant::now() + Duration::from_secs(30);
    publish(dir, "root.ready.json", &json!({"pid":std::process::id()}))?;
    gate(dir, "go", overall)?; // Parent observes production enrollment before any agent socket.
    let i = args
        .iter()
        .position(|s| s == "--mcp-config")
        .ok_or("actual mcp config flag missing")?;
    let config: Value = serde_json::from_slice(&std::fs::read(
        args.get(i + 1).ok_or("mcp config path missing")?,
    )?)?;
    let argv = config["mcpServers"]["crew"]["args"]
        .as_array()
        .ok_or("generated helper args missing")?;
    let argument = |flag: &str| -> Result<String, Box<dyn std::error::Error>> {
        let i = argv
            .iter()
            .position(|v| v.as_str() == Some(flag))
            .ok_or("generated helper flag missing")?;
        Ok(argv
            .get(i + 1)
            .and_then(Value::as_str)
            .ok_or("generated helper value missing")?
            .to_string())
    };
    let socket = Path::new(&argument("--data-dir")?).join("crewd.sock");
    let session = argument("--session")?;
    let token = std::fs::read_to_string(argument("--token-file")?)?;
    let stream = UnixStream::connect(socket)?;
    let mut wire = Wire {
        writer: stream.try_clone()?,
        reader: BufReader::new(stream),
        events: Vec::new(),
        overall,
    };
    // helperBuild optional in S6: no fabricated build identity is supplied. Actual attach remains legal.
    let attach = wire.call(
        90101,
        "agent.attach",
        json!({"sessionId":session,"token":token.trim()}),
    )?;
    let tools = wire.call(90102, "agent.tools", Value::Null)?;
    publish(
        dir,
        "attached.complete.json",
        &json!({"session":session,"attach":attach,"tools":tools}),
    )?;
    gate(dir, "fault.go", overall)?;
    // Preserve a real transport failure as an observation, not an early fixture panic.
    // Parent then requests finish and checks the numbered Internal oracle after cleanup.
    let fault = match wire.call(90103, "agent.tools", Value::Null) {
        Ok(response) => json!({"fault":response}),
        Err(error) => json!({"fault":Value::Null,"transport_error":error.to_string()}),
    };
    publish(dir, "fault.complete.json", &fault)?;
    let deadline = (Instant::now() + Duration::from_secs(15)).min(overall);
    loop {
        if dir.join("finish").is_file() {
            drop(wire);
            return Ok(());
        }
        if dir.join("role.go").is_file() {
            break;
        }
        if Instant::now() >= deadline {
            return Err("post-fault disposition deadline".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    wire.role_event()?;
    let after_tools = wire.call(90104, "agent.tools", Value::Null)?;
    let reattach = wire.call(
        90105,
        "agent.attach",
        json!({"sessionId":session,"token":token.trim()}),
    )?;
    let repeated_tools = wire.call(90106, "agent.tools", Value::Null)?;
    let spec: Value = serde_json::from_slice(&std::fs::read(dir.join("spec.json"))?)?;
    let denied = wire.call(
        90107,
        "message.post",
        json!({"channelId":spec["channel"],"text":"S5_NO_HUMAN_UPGRADE","mentions":[]}),
    )?;
    wire.finish_socket()?; // Actual final EOF includes the watcher tail; no silence timer.
    publish(
        dir,
        "complete.json",
        &json!({"session":session,"attach":attach,"before_tools":tools,"fault":fault["fault"],
        "after_tools":after_tools,"reattach":reattach,"repeated_tools":repeated_tools,
        "denied":denied,"events":wire.events}),
    )?;
    // Socket is gone before parent finishes the original driver-owned root.
    drop(wire);
    publish(
        dir,
        "socket.closed.json",
        &json!({"actual_eof":true,"root_still_live":true}),
    )?;
    gate(dir, "finish", overall)?;
    Ok(())
}
