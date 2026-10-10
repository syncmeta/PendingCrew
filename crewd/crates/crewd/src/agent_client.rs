//! helper 侧的 crewd 连接：连 `<data dir>/crewd.sock`、`agent.attach`，之后一条连接上并发多个请求
//! （rmcp 可能同时 list 与 call），回应按 id 分发，`agent.tools_changed` 之类的推送进一个通道。
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, anyhow};
use crewd_proto::agent::{AttachParams, AttachResult, methods};
use crewd_proto::{Event, Request, Response, ServerFrame};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::{Mutex, mpsc, oneshot};

type Pending = Arc<std::sync::Mutex<Option<HashMap<u64, oneshot::Sender<Response>>>>>;

pub struct AgentClient {
    wr: Mutex<OwnedWriteHalf>,
    pending: Pending,
    next: AtomicU64,
    events: std::sync::Mutex<Option<mpsc::UnboundedReceiver<Event>>>,
}

impl AgentClient {
    pub async fn connect(
        data_dir: &Path,
        session: &str,
        token_file: &Path,
    ) -> anyhow::Result<(AgentClient, AttachResult)> {
        let token = read_token(token_file).await?;
        let sock = data_dir.join("crewd.sock");
        let stream = UnixStream::connect(&sock)
            .await
            .with_context(|| format!("cannot connect to {}", sock.display()))?;
        let (rd, wr) = stream.into_split();
        let pending: Pending = Arc::new(std::sync::Mutex::new(Some(HashMap::new())));
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        let p2 = pending.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(rd).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                match serde_json::from_str::<ServerFrame>(&line) {
                    Ok(ServerFrame::Response(r)) => {
                        let tx = p2.lock().unwrap().as_mut().and_then(|m| m.remove(&r.id));
                        if let Some(tx) = tx {
                            let _ = tx.send(r);
                        }
                    }
                    Ok(ServerFrame::Event(e)) => {
                        let _ = ev_tx.send(e);
                    }
                    Err(e) => eprintln!("crewd agent client: bad frame from crewd: {e}"),
                }
            }
            // 断开：清空等待中的请求（它们会收到「连接关了」），之后的请求直接失败。
            p2.lock().unwrap().take();
        });
        let client = AgentClient {
            wr: Mutex::new(wr),
            pending,
            next: AtomicU64::new(1),
            events: std::sync::Mutex::new(Some(ev_rx)),
        };
        let who: AttachResult = client
            .call(
                methods::ATTACH,
                serde_json::to_value(AttachParams {
                    session_id: session.to_string(),
                    token,
                })?,
            )
            .await?;
        Ok((client, who))
    }

    pub async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> Result<T, CallError> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        match self.pending.lock().unwrap().as_mut() {
            Some(m) => {
                m.insert(id, tx);
            }
            None => return Err(CallError::NotSent("connection to crewd is closed".into())),
        }
        let mut line = serde_json::to_vec(&Request {
            id,
            method: method.to_string(),
            params,
        })
        .map_err(|e| CallError::NotSent(format!("cannot encode the request: {e}")))?;
        line.push(b'\n');
        // 写失败也算「可能已送达」：半行在对端 EOF 时也可能被当成一整行读走。
        self.wr
            .lock()
            .await
            .write_all(&line)
            .await
            .map_err(|e| CallError::Lost(format!("write to crewd failed: {e}")))?;
        let resp = rx
            .await
            .map_err(|_| CallError::Lost("connection to crewd closed before it answered".into()))?;
        // M3a 的 `Response` 是 `{ id, outcome }`：`into_result()` 区分「成功（值可以是 null）」与错误。
        let v = resp.into_result().map_err(|e| CallError::Rejected {
            code: e.code,
            message: e.message,
        })?;
        serde_json::from_value(v).map_err(|e| CallError::BadReply(e.to_string()))
    }

    pub fn take_events(&self) -> Option<mpsc::UnboundedReceiver<Event>> {
        self.events.lock().unwrap().take()
    }
}

/// 一次调用失败在哪一步：调用方（`crewd mcp`）据此告诉 agent「没做」「被拒」还是「不知道做没做」。
#[derive(Debug)]
pub enum CallError {
    /// 请求没发出去：对端一定没做。
    NotSent(String),
    /// crewd 回了错误：这次调用被拒。
    Rejected { code: i32, message: String },
    /// 请求已写出（或写到一半），之后连接断了：可能做了也可能没做。
    Lost(String),
    /// crewd 回了成功，但结果读不懂：已经做了。
    BadReply(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSent(m) => write!(f, "not sent: {m}"),
            Self::Rejected { code, message } => write!(f, "{message} ({code})"),
            Self::Lost(m) => write!(f, "{m}"),
            Self::BadReply(m) => write!(f, "unreadable answer from crewd: {m}"),
        }
    }
}

impl std::error::Error for CallError {}

/// 在自己的线程里读 token 文件：路径可能是一个没人写的 FIFO、或卡住的文件系统——那会卡住
/// 当前 tokio 工作线程，调用方的期限（hook 的 5s）也就永远到不了。线程不 join，进程退出时随之结束。
async fn read_token(path: &Path) -> anyhow::Result<String> {
    let p = path.to_path_buf();
    let (tx, rx) = oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(std::fs::read_to_string(&p));
    });
    let raw = rx
        .await
        .map_err(|_| anyhow!("token reader thread ended without an answer"))?
        .with_context(|| format!("cannot read token file {}", path.display()))?;
    Ok(raw.trim().to_string())
}
