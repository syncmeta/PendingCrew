//! Validate new V2 tool frames before rmcp converts arguments to a JSON map.
//! All accepted input is forwarded byte-for-byte to the existing SDK codec;
//! SDK-owned output serialization and its shared writer mutex remain in use.
use crewd_server::managed_v2_input::{is_v2_tool, unique_fields};
use rmcp::{
    RoleServer,
    model::{ErrorData, RequestId},
    service::{RxJsonRpcMessage, TxJsonRpcMessage},
    transport::{Transport, async_rw::AsyncRwTransport},
};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll, ready},
};
use tokio::{
    io::{AsyncBufRead, AsyncRead, AsyncWrite, BufReader, ReadBuf},
    sync::mpsc,
};

struct FilteredInput<R> {
    read: BufReader<R>,
    line: Vec<u8>,
    output: Vec<u8>,
    offset: usize,
    eof: bool,
    rejected: mpsc::UnboundedSender<Option<RequestId>>,
}

fn rejected_frame(raw: &[u8]) -> Option<Option<RequestId>> {
    // Match the SDK's accepted detection view without altering forwarded bytes.
    let raw = raw.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(raw);
    let value: serde_json::Value = serde_json::from_slice(raw).ok()?;
    if value.get("method").and_then(serde_json::Value::as_str) != Some("tools/call")
        || !value
            .pointer("/params/name")
            .and_then(serde_json::Value::as_str)
            .is_some_and(is_v2_tool)
        || unique_fields(std::str::from_utf8(raw).ok()?).is_ok()
    {
        return None;
    }
    Some(
        value
            .get("id")
            .and_then(|id| serde_json::from_value(id.clone()).ok()),
    )
}

impl<R: AsyncRead + Unpin> AsyncRead for FilteredInput<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if out.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            if this.offset < this.output.len() {
                let end = (this.offset + out.remaining()).min(this.output.len());
                out.put_slice(&this.output[this.offset..end]);
                this.offset = end;
                if end == this.output.len() {
                    this.output.clear();
                    this.offset = 0;
                }
                return Poll::Ready(Ok(()));
            }
            if this.eof && this.line.is_empty() {
                return Poll::Ready(Ok(()));
            }
            let complete = if this.eof {
                true
            } else {
                let bytes = ready!(Pin::new(&mut this.read).poll_fill_buf(cx))?;
                if bytes.is_empty() {
                    this.eof = true;
                    !this.line.is_empty()
                } else {
                    let newline = bytes.iter().position(|byte| *byte == b'\n');
                    let used = newline.map_or(bytes.len(), |index| index + 1);
                    this.line.extend_from_slice(&bytes[..used]);
                    Pin::new(&mut this.read).consume(used);
                    newline.is_some()
                }
            };
            if !complete {
                continue;
            }
            if let Some(id) = rejected_frame(&this.line) {
                this.line.clear();
                if this.rejected.send(id).is_err() {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "MCP input receiver closed",
                    )));
                }
                // Yield after one rejection. receive() drains it before polling
                // another frame, so malicious input cannot fill an unbounded queue.
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            this.output = std::mem::take(&mut this.line);
        }
    }
}

pub(super) struct V2Transport<R: AsyncRead + Unpin, W: AsyncWrite> {
    inner: AsyncRwTransport<RoleServer, FilteredInput<R>, W>,
    rejected: mpsc::UnboundedReceiver<Option<RequestId>>,
    pending_rejection: Option<Pin<Box<dyn Future<Output = io::Result<()>> + Send>>>,
}
impl<R, W> V2Transport<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send + 'static,
{
    pub(super) fn new(read: R, write: W) -> Self {
        let (sender, rejected) = mpsc::unbounded_channel();
        let input = FilteredInput {
            read: BufReader::new(read),
            line: Vec::new(),
            output: Vec::new(),
            offset: 0,
            eof: false,
            rejected: sender,
        };
        Self {
            inner: AsyncRwTransport::new_server(input, write),
            rejected,
            pending_rejection: None,
        }
    }
}
impl<R, W> Transport<RoleServer> for V2Transport<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send + 'static,
{
    type Error = io::Error;
    fn send(
        &mut self,
        message: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.inner.send(message)
    }
    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        loop {
            if let Some(pending) = &mut self.pending_rejection {
                let result = pending.as_mut().await;
                self.pending_rejection = None;
                if result.is_err() {
                    return None;
                }
            }
            tokio::select! {
                biased;
                Some(id) = self.rejected.recv() => {
                    let error = TxJsonRpcMessage::<RoleServer>::error(ErrorData::invalid_params("Duplicate object fields are not valid V2 input", None), id);
                    // rmcp cancels receive() when another service branch wins.
                    // Retain this exact SDK send future, including partial flush,
                    // in the transport so cancellation cannot lose or duplicate it.
                    self.pending_rejection = Some(Box::pin(self.inner.send(error)));
                }
                message = self.inner.receive() => return message,
            }
        }
    }
    async fn close(&mut self) -> Result<(), Self::Error> {
        // A closed transport may abandon its output, but must release a writer
        // guard held by the retained send before requesting SDK closure.
        self.pending_rejection = None;
        self.inner.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn duplicate_v2_frame_returns_correlated_error_then_original_uuid_remains_usable() {
        let (mut input, reader) = tokio::io::duplex(8192);
        let (writer, output) = tokio::io::duplex(8192);
        let task = tokio::spawn(async move {
            let mut transport = V2Transport::new(reader, writer);
            transport.receive().await.unwrap()
        });
        input.write_all(br#"{"jsonrpc":"2.0","id":41,"method":"tools/call","params":{"name":"managed_table_v2_submit","arguments":{"requestId":"original","operation":{"kind":"addRow","kind":"createTable"}}}}
"#).await.unwrap();
        let mut output = BufReader::new(output);
        let mut line = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            output.read_line(&mut line),
        )
        .await
        .unwrap()
        .unwrap();
        let rejected: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(rejected["id"], 41);
        assert_eq!(rejected["error"]["code"], -32602);
        let accepted = json!({"jsonrpc":"2.0","id":42,"method":"tools/call","params":{"name":"managed_table_v2_submit","arguments":{"requestId":"original","operation":{"kind":"createTable"}}}});
        input
            .write_all(format!("{accepted}\n").as_bytes())
            .await
            .unwrap();
        let actual = tokio::time::timeout(std::time::Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(serde_json::to_value(actual).unwrap(), accepted);
    }

    #[test]
    fn old_tools_and_json_strings_keep_existing_sdk_input_behavior() {
        assert!(rejected_frame(br#"{"id":1,"method":"tools/call","params":{"name":"managed_object_submit","arguments":{"x":1,"x":2}}}"#).is_none());
        assert!(rejected_frame(br#"{"id":1,"method":"tools/call","params":{"name":"managed_table_v2_get","arguments":{"id":"{\"x\":1,\"x\":2}"}}}"#).is_none());
        assert!(rejected_frame(br#"{"id":1,"method":"tools/call","params":{"name":"managed_link_resolve_target","arguments":{"target":{"kind":"human","\u006b\u0069nd":"human"}}}}"#).is_some());
    }

    #[test]
    fn bom_uses_the_same_detection_view_as_the_sdk() {
        let mut bad = vec![0xef, 0xbb, 0xbf];
        bad.extend_from_slice(br#"{"id":7,"method":"tools/call","params":{"name":"managed_link_resolve_target","arguments":{"target":{"kind":"human","\u006b\u0069nd":"human"}}}}"#);
        assert!(rejected_frame(&bad).is_some());
        let mut good = vec![0xef, 0xbb, 0xbf];
        good.extend_from_slice(br#"{"id":8,"method":"tools/call","params":{"name":"managed_table_v2_get","arguments":{"id":"T1"}}}"#);
        assert!(rejected_frame(&good).is_none());
    }

    #[tokio::test]
    async fn fragmented_bom_crlf_and_unterminated_eof_preserve_exact_accepted_bytes() {
        use tokio::io::AsyncReadExt;
        let (mut input, reader) = tokio::io::duplex(8192);
        let (sender, mut rejected) = mpsc::unbounded_channel();
        let mut filter = FilteredInput {
            read: BufReader::new(reader),
            line: Vec::new(),
            output: Vec::new(),
            offset: 0,
            eof: false,
            rejected: sender,
        };
        let raw = b"\xef\xbb\xbf{\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"managed_table_v2_get\",\"arguments\":{\"id\":\"T1\"}}}\r\n{\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"managed_object_submit\",\"arguments\":{\"x\":1,\"x\":2}}}";
        input.write_all(&raw[..19]).await.unwrap();
        let mut bytes = Vec::new();
        {
            let mut read = Box::pin(filter.read_to_end(&mut bytes));
            std::future::poll_fn(|cx| {
                assert!(read.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
        }
        assert_eq!(filter.line, raw[..19]);
        assert!(bytes.is_empty());
        input.write_all(&raw[19..]).await.unwrap();
        input.shutdown().await.unwrap();
        filter.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, raw);
        assert!(matches!(
            rejected.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn cancelled_receive_retains_one_rejection_send_across_writer_backpressure() {
        use tokio::io::AsyncReadExt;
        let (mut input, reader) = tokio::io::duplex(8192);
        let (writer, mut output) = tokio::io::duplex(64);
        let mut transport = V2Transport::new(reader, writer);
        let normal = TxJsonRpcMessage::<RoleServer>::error(
            ErrorData::internal_error("x".repeat(4096), None),
            Some(serde_json::from_value(json!(10)).unwrap()),
        );
        let mut blocked = Box::pin(transport.send(normal));
        std::future::poll_fn(|cx| {
            assert!(blocked.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        input.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":41,\"method\":\"tools/call\",\"params\":{\"name\":\"managed_table_v2_get\",\"arguments\":{\"id\":\"T1\",\"id\":\"T2\"}}}\n").await.unwrap();
        for _ in 0..2 {
            let mut receive = Box::pin(transport.receive());
            std::future::poll_fn(|cx| {
                assert!(receive.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
        }
        assert!(
            transport.pending_rejection.is_some(),
            "cut must follow dequeue and precede writer acquisition"
        );
        assert!(transport.rejected.is_empty());
        input.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":42,\"method\":\"tools/call\",\"params\":{\"name\":\"managed_table_v2_get\",\"arguments\":{\"id\":\"T1\"}}}\n").await.unwrap();
        let collected = tokio::spawn(async move {
            let mut bytes = Vec::new();
            output.read_to_end(&mut bytes).await.unwrap();
            bytes
        });
        let (sent, received) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            tokio::join!(blocked, transport.receive())
        })
        .await
        .unwrap();
        sent.unwrap();
        assert_eq!(serde_json::to_value(received.unwrap()).unwrap()["id"], 42);
        transport.close().await.unwrap();
        let bytes = tokio::time::timeout(std::time::Duration::from_secs(3), collected)
            .await
            .unwrap()
            .unwrap();
        let frames: Vec<serde_json::Value> = String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(frames.len(), 2);
        assert_eq!(
            frames
                .iter()
                .filter(|frame| frame["id"] == 41 && frame["error"]["code"] == -32602)
                .count(),
            1
        );
    }
}
