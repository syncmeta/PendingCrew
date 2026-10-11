//! Opt-in owned synthetic raw wire client; no provider, declared authority or retry.
use serde_json::{Value, json};
use std::{
    io::{self, Read, Write},
    net::Shutdown,
    os::fd::AsRawFd,
    os::unix::{fs::OpenOptionsExt, net::UnixStream},
    path::Path,
    time::{Duration, Instant},
};
pub fn run(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let config: Value = serde_json::from_slice(&std::fs::read(dir.join("request.json"))?)?;
    let packet = &config["request"];
    let id = packet["id"].as_u64().ok_or("synthetic request id")?;
    let stream = UnixStream::connect(config["socket"].as_str().ok_or("synthetic socket")?)?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    let mut writer = stream.try_clone()?;
    let mut reader = stream;
    let sent = Instant::now();
    writeln!(writer, "{packet}")?;
    let mut frames = Vec::new();
    let mut half_closed = false;
    let mut response_elapsed = None;
    let observed = (|| -> Result<bool, Box<dyn std::error::Error>> {
        let deadline = sent + Duration::from_secs(3);
        // One flag change after the single packet write; no setsockopt after disconnect.
        reader.set_nonblocking(true)?;
        let mut pending = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "synthetic response/EOF deadline",
                )
                .into());
            }
            let millis = left.as_nanos().div_ceil(1_000_000).min(i32::MAX as u128) as i32;
            let mut fd = libc::pollfd {
                fd: reader.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // The live owned UnixStream holds fd; poll accesses this one initialized stack entry.
            let ready = unsafe { libc::poll(&mut fd, 1, millis) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error.into());
            }
            if ready == 0 {
                continue;
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "synthetic response/EOF deadline",
                )
                .into());
            }
            if fd.revents & libc::POLLNVAL != 0 {
                return Err(io::Error::from_raw_os_error(libc::EBADF).into());
            }
            // HUP/ERR flags are never EOF evidence. Only an actual read returning zero is EOF.
            let n = match reader.read(&mut chunk) {
                Ok(n) => n,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            if n == 0 {
                if !pending.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "synthetic truncated NDJSON frame",
                    )
                    .into());
                }
                return Ok(true);
            }
            pending.extend_from_slice(&chunk[..n]);
            if pending.len() > 16 * 1024 * 1024 + 1 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "synthetic NDJSON frame exceeds wire bound",
                )
                .into());
            }
            while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                let line = pending.drain(..=end).collect::<Vec<_>>();
                let frame: Value = serde_json::from_slice(&line)?;
                let matches = frame["id"].as_u64() == Some(id);
                frames.push(frame);
                if matches {
                    response_elapsed = Some(sent.elapsed().as_millis());
                    if config["halfCloseAfterResponse"].as_bool() == Some(true) && !half_closed {
                        writer.shutdown(Shutdown::Write)?;
                        half_closed = true;
                    }
                }
            }
        }
    })();
    let error = observed.as_ref().err().map(|e| e.to_string());
    let value = json!({"pid":std::process::id(),"frames":frames,"eof":matches!(&observed,Ok(true)),
        "responseElapsedMs":response_elapsed,"halfClosed":half_closed,"error":error});
    let mut receipt = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dir.join("wire.receipt.json"))?;
    writeln!(receipt, "{value}")?;
    receipt.sync_all()?;
    observed?;
    Ok(())
}
