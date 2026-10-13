//! ACP control NDJSON and private native terminal transport are independent.
//! Neither decoder includes untrusted payloads in errors, or accumulates an unbounded line.
use super::wire::{
    self, Control, HEADER_LEN, MAX_CONTROL, MAX_TERMINAL_PAYLOAD, TerminalDecoder, TerminalFrame,
};
use std::io;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

pub struct ControlReader<R> {
    reader: BufReader<R>,
    pending: Vec<u8>,
    poisoned: bool,
}
impl<R: AsyncRead + Unpin> ControlReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
            pending: Vec::new(),
            poisoned: false,
        }
    }
    pub async fn read(&mut self) -> io::Result<Option<Control>> {
        if self.poisoned {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "closed ACP control decoder",
            ));
        }
        let result = self.read_next().await;
        if result.is_err() {
            self.poisoned = true;
            self.pending.clear();
        }
        result
    }
    async fn read_next(&mut self) -> io::Result<Option<Control>> {
        loop {
            let chunk = self.reader.fill_buf().await?;
            if chunk.is_empty() {
                return if self.pending.is_empty() {
                    Ok(None)
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "incomplete ACP control frame",
                    ))
                };
            }
            let end = chunk.iter().position(|b| *b == b'\n');
            let n = end.map_or(chunk.len(), |n| n + 1);
            if self.pending.len().saturating_add(n) > MAX_CONTROL {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "ACP control frame limit",
                ));
            }
            self.pending.extend_from_slice(&chunk[..n]);
            self.reader.consume(n);
            if end.is_some() {
                let bytes = std::mem::take(&mut self.pending);
                return wire::control(&bytes).map(Some).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid ACP control frame")
                });
            }
        }
    }
}
struct BoundedJson(Vec<u8>);
impl io::Write for BoundedJson {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if self.0.len().saturating_add(b.len()) >= MAX_CONTROL {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ACP control frame limit",
            ));
        }
        self.0.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub async fn write_control<W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: &serde_json::Value,
) -> io::Result<()> {
    let mut buffer = BoundedJson(Vec::new());
    serde_json::to_writer(&mut buffer, value).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid outbound ACP control frame",
        )
    })?;
    wire::control(&buffer.0).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid outbound ACP control frame",
        )
    })?;
    buffer.0.push(b'\n');
    writer.write_all(&buffer.0).await?;
    writer.flush().await
}

/// A cancelled read retains every consumed header/payload byte. Invalid input
/// closes this stream permanently; it cannot be reparsed at a different boundary.
pub struct TerminalReader<R> {
    reader: BufReader<R>,
    pending: Vec<u8>,
    total: Option<usize>,
    poisoned: bool,
}
impl<R: AsyncRead + Unpin> TerminalReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
            pending: Vec::new(),
            total: None,
            poisoned: false,
        }
    }
    pub async fn read(&mut self) -> io::Result<Option<TerminalFrame>> {
        if self.poisoned {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "closed terminal decoder",
            ));
        }
        let result = self.read_next().await;
        if result.is_err() {
            self.poisoned = true;
            self.pending.clear();
        }
        result
    }
    async fn read_next(&mut self) -> io::Result<Option<TerminalFrame>> {
        loop {
            let limit = self.total.unwrap_or(HEADER_LEN);
            if self.pending.len() == limit {
                if self.total.is_none() {
                    if &self.pending[..4] != b"PCPT" {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "terminal stream mismatch",
                        ));
                    }
                    let n = u32::from_be_bytes(self.pending[4..8].try_into().expect("fixed header"))
                        as usize;
                    if n > MAX_TERMINAL_PAYLOAD {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "terminal frame limit",
                        ));
                    }
                    self.total = Some(HEADER_LEN + n);
                    if n != 0 {
                        continue;
                    }
                }
                let bytes = std::mem::take(&mut self.pending);
                self.total = None;
                let mut frames = TerminalDecoder::default().push(&bytes).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid terminal frame")
                })?;
                if frames.len() != 1 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "incomplete terminal frame",
                    ));
                }
                return Ok(frames.pop());
            }
            let chunk = self.reader.fill_buf().await?;
            if chunk.is_empty() {
                return if self.pending.is_empty() {
                    Ok(None)
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "incomplete terminal frame",
                    ))
                };
            }
            let n = chunk.len().min(limit - self.pending.len());
            self.pending.extend_from_slice(&chunk[..n]);
            self.reader.consume(n);
        }
    }
}
pub async fn write_terminal<W: AsyncWrite + Unpin>(
    writer: &mut W,
    frame: &TerminalFrame,
) -> io::Result<()> {
    let bytes = frame
        .encode()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid terminal frame"))?;
    writer.write_all(&bytes).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;
    use tokio::net::UnixStream;
    #[tokio::test]
    async fn fake_acp_peer_cancels_while_independent_terminal_pipe_is_backpressured() {
        let (mut client, server) = UnixStream::pair().unwrap();
        let (mut producer, _stalled_viewer) = UnixStream::pair().unwrap();
        let flooding = tokio::spawn(async move {
            for seq in 1..=100 {
                write_terminal(
                    &mut producer,
                    &TerminalFrame {
                        epoch: 1,
                        sequence: seq,
                        kind: wire::TerminalKind::Data,
                        bytes: vec![b'x'; MAX_TERMINAL_PAYLOAD],
                    },
                )
                .await
                .unwrap();
            }
        });
        let peer = tokio::spawn(async move {
            let (read, mut write) = server.into_split();
            let mut reader = ControlReader::new(read);
            assert!(
                matches!(reader.read().await.unwrap(), Some(Control::Request { method, .. }) if method == "_pendingcrew.native/stop")
            );
            write_control(
                &mut write,
                &json!({"jsonrpc":"2.0","id":1,"result":{"accepted":true}}),
            )
            .await
            .unwrap();
        });
        write_control(
            &mut client,
            &json!({"jsonrpc":"2.0","id":1,"method":"_pendingcrew.native/stop","params":{}}),
        )
        .await
        .unwrap();
        let mut reader = ControlReader::new(client);
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), reader.read())
                .await
                .unwrap()
                .unwrap(),
            Some(Control::Response { result: Ok(_), .. })
        ));
        assert!(
            !flooding.is_finished(),
            "terminal consumer really stayed stalled"
        );
        flooding.abort();
        let _ = flooding.await;
        peer.await.unwrap();
    }
    #[tokio::test]
    async fn cancelled_control_read_retains_partial_frame_and_malformed_poison_is_permanent() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        let mut decoder = ControlReader::new(reader);
        writer
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":7,")
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), decoder.read())
                .await
                .is_err()
        );
        writer
            .write_all(b"\"method\":\"session/cancel\",\"params\":{}}\n")
            .await
            .unwrap();
        assert!(
            matches!(decoder.read().await.unwrap(), Some(Control::Request { id, method, .. })
            if id == json!(7) && method == "session/cancel")
        );
        writer
            .write_all(b"invalid\n{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"session/new\"}\n")
            .await
            .unwrap();
        assert_eq!(
            decoder.read().await.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            decoder.read().await.unwrap_err().to_string(),
            "closed ACP control decoder"
        );
    }

    #[tokio::test]
    async fn cancelled_terminal_read_retains_header_and_payload_and_poison_is_permanent() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        let mut decoder = TerminalReader::new(reader);
        let frame = TerminalFrame {
            epoch: 5,
            sequence: 1,
            kind: wire::TerminalKind::Data,
            bytes: "原生🙂".as_bytes().to_vec(),
        };
        let bytes = frame.encode().unwrap();
        writer.write_all(&bytes[..7]).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), decoder.read())
                .await
                .is_err()
        );
        writer.write_all(&bytes[7..HEADER_LEN + 2]).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), decoder.read())
                .await
                .is_err()
        );
        writer.write_all(&bytes[HEADER_LEN + 2..]).await.unwrap();
        assert_eq!(decoder.read().await.unwrap(), Some(frame));
        let invalid = vec![0; HEADER_LEN];
        writer.write_all(&invalid).await.unwrap();
        assert_eq!(
            decoder.read().await.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        writer.write_all(&bytes).await.unwrap();
        assert_eq!(
            decoder.read().await.unwrap_err().to_string(),
            "closed terminal decoder"
        );
    }

    #[tokio::test]
    async fn terminal_frames_preserve_opaque_utf8_and_control_rejects_unterminated_or_oversize_lines()
     {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        let frame = TerminalFrame {
            epoch: 4,
            sequence: 1,
            kind: wire::TerminalKind::Data,
            bytes: "\x1b[32m原生🙂".as_bytes().to_vec(),
        };
        write_terminal(&mut writer, &frame).await.unwrap();
        assert_eq!(
            TerminalReader::new(reader).read().await.unwrap(),
            Some(frame)
        );
        let (mut writer, reader) = UnixStream::pair().unwrap();
        writer.write_all(b"{\"jsonrpc\":\"2.0\"").await.unwrap();
        writer.shutdown().await.unwrap();
        assert_eq!(
            ControlReader::new(reader).read().await.unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        let oversized = vec![b'x'; MAX_CONTROL + 1];
        assert_eq!(
            ControlReader::new(oversized.as_slice())
                .read()
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}
