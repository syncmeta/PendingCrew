//! helper 侧的 crewd 连接：连 `<data dir>/crewd.sock`、`agent.attach`，之后一条连接上并发多个请求
//! （rmcp 可能同时 list 与 call），回应按 id 分发，`agent.tools_changed` 之类的推送进一个通道。
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, anyhow, bail};
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
        let token = std::fs::read_to_string(token_file)
            .with_context(|| format!("cannot read token file {}", token_file.display()))?
            .trim()
            .to_string();
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
    ) -> anyhow::Result<T> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        match self.pending.lock().unwrap().as_mut() {
            Some(m) => {
                m.insert(id, tx);
            }
            None => bail!("connection to crewd is closed"),
        }
        let mut line = serde_json::to_vec(&Request {
            id,
            method: method.to_string(),
            params,
        })?;
        line.push(b'\n');
        self.wr
            .lock()
            .await
            .write_all(&line)
            .await
            .context("write to crewd")?;
        let resp = rx
            .await
            .map_err(|_| anyhow!("connection to crewd is closed"))?;
        // M3a 的 `Response` 是 `{ id, outcome }`：`into_result()` 区分「成功（值可以是 null）」与错误。
        let v = resp
            .into_result()
            .map_err(|e| anyhow!("{} ({})", e.message, e.code))?;
        Ok(serde_json::from_value(v)?)
    }

    pub fn take_events(&self) -> Option<mpsc::UnboundedReceiver<Event>> {
        self.events.lock().unwrap().take()
    }
}
