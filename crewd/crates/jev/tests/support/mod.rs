//! 假 Jev：本机 TCP 上的极简 HTTP/1.1 服务器。按顺序吐出预设的应答，记下收到的每个请求。
//! 每个连接只处理一个请求并 `connection: close`，免得客户端复用连接让计数对不上。
#![allow(dead_code)]
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub delay: Duration,
}

impl Reply {
    pub fn ok(body: impl Into<String>) -> Reply {
        Reply {
            status: 200,
            headers: vec![],
            body: body.into(),
            delay: Duration::ZERO,
        }
    }

    pub fn status(status: u16) -> Reply {
        Reply {
            status,
            headers: vec![],
            body: format!("{{\"error\":\"fake {status}\"}}"),
            delay: Duration::ZERO,
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Reply {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn body(mut self, body: impl Into<String>) -> Reply {
        self.body = body.into();
        self
    }

    pub fn delayed(mut self, delay: Duration) -> Reply {
        self.delay = delay;
        self
    }
}

#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    /// 头名一律小写。
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

pub struct FakeJev {
    pub base_url: String,
    recorded: Arc<Mutex<Vec<Recorded>>>,
}

impl FakeJev {
    /// 应答用完以后再来的请求一律 500（测试里多出来的请求会因此显形）。
    pub async fn start(replies: Vec<Reply>) -> FakeJev {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let queue = Arc::new(Mutex::new(VecDeque::from(replies)));
        let rec = recorded.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let rec = rec.clone();
                let queue = queue.clone();
                tokio::spawn(async move {
                    let Some(req) = read_request(&mut sock).await else {
                        return;
                    };
                    rec.lock().unwrap().push(req);
                    let reply = queue
                        .lock()
                        .unwrap()
                        .pop_front()
                        .unwrap_or_else(|| Reply::status(500).body("fake: no more replies"));
                    tokio::time::sleep(reply.delay).await;
                    let mut head = format!(
                        "HTTP/1.1 {} Fake\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
                        reply.status,
                        reply.body.len()
                    );
                    for (k, v) in &reply.headers {
                        head.push_str(&format!("{k}: {v}\r\n"));
                    }
                    head.push_str("\r\n");
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(reply.body.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        FakeJev {
            base_url: format!("http://127.0.0.1:{port}"),
            recorded,
        }
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.recorded.lock().unwrap().clone()
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

pub(crate) async fn read_request(sock: &mut TcpStream) -> Option<Recorded> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = find(&buf, b"\r\n\r\n") {
            break pos;
        }
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_string();
    let path = request_line.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let len: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < len {
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Recorded {
        method,
        path,
        headers,
        body,
    })
}

/// 合法的 choice 应答（选 `choice`，置信度 `confidence`）。
pub fn choice_body(question: &str, choice: &str, confidence: f64) -> String {
    serde_json::json!({
        "model": "jev-1.13.0",
        "answers": { question: {
            "type": "choice",
            "choice": choice,
            "probabilities": { choice: 1.0 },
            "confidence": confidence
        }},
        "usage": { "input_tokens": 300, "output_tokens": 20 }
    })
    .to_string()
}

/// 合法的 noul 应答。
pub fn noul_body(ps: &[(&str, f64)]) -> String {
    let answers: serde_json::Map<String, serde_json::Value> = ps
        .iter()
        .map(|(k, p)| {
            (
                k.to_string(),
                serde_json::json!({"type": "noul", "noul": p}),
            )
        })
        .collect();
    serde_json::json!({ "model": "jev-1.13.0", "answers": answers }).to_string()
}
