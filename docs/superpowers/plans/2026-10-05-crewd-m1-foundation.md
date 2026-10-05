# crewd M1 地基 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 建起 Rust 后端 crewd 的地基：workspace、SQLite 存储、旧 JSON 数据只读导入、Unix socket 上的 NDJSON RPC（含事件订阅）、可提交的 API 契约 schema、`crewd` 命令行。

**Architecture:** `crewd/` 是仓库根下的 Cargo workspace，五个 crate：`proto`（线协议类型，唯一契约）→ `store`（SQLite，crewd 唯一写者）→ `legacy`（旧数据导入，只读源目录）→ `server`（socket + 分发 + 事件 hub）→ `crewd`（二进制 CLI）。依赖只能沿此方向。M1 不碰 Swift app、不碰 PTY。

**Tech Stack:** Rust 1.94（edition 2024）、rusqlite（bundled）、tokio、serde/serde_json、schemars、chrono、clap、dirs、uuid、thiserror/anyhow；测试用 tempfile。

**Spec:** `docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`（§3.3 M1、§3.4、§3.5）

## Global Constraints

- 所有命令在 `crewd/` 目录下跑；Rust 工具链在 `/opt/homebrew/opt/rustup/bin`（已加进 `~/.zshrc`）。若 `cargo` 找不到，先 `export PATH=/opt/homebrew/opt/rustup/bin:$PATH`。
- 依赖一律用 `cargo add` 加（取当时最新版），**不要手写版本号**；`Cargo.lock` 要提交。
- 线协议字段一律 camelCase（`#[serde(rename_all = "camelCase")]`），可选字段 `skip_serializing_if = "Option::is_none"`。
- 时间戳统一存成 RFC3339 UTC 毫秒格式，例：`2026-09-11T04:34:56.789Z`（`chrono` 的 `to_rfc3339_opts(SecondsFormat::Millis, true)`）。
- 旧数据目录**只读**：导入代码绝不写、改、删源目录里的任何文件。
- 测试**只用合成数据**（仓库是公开的）。真实数据只在 Task 10 的人工步骤里读，且只看计数报告。
- 数据目录：`--data-dir` > 环境变量 `CREWD_DATA_DIR` > `dirs::data_dir()/crewd`（macOS 即 `~/Library/Application Support/crewd`）。库文件 `crewd.db`，socket `crewd.sock`，锁 `crewd.lock`。
- 旧数据目录：`--from` > 环境变量 `PENDINGCREW_DATA_DIR`（trim，`~/` 展开）> `dirs::data_dir()/PendingCrew`。
- 每个任务结束跑 `crewd/scripts/check.sh`（fmt + clippy -D warnings + test）全绿再提交。提交只 `git add` 该任务列出的文件，提交信息末尾加 `Co-Authored-By: Claude <noreply@anthropic.com>`。
- 在分支 `crewd-m1` 上做（执行时用 worktree）。

## Review Focus

1. 旧账本里有坏行（缺必填字段）、整个文件不是合法 JSON → 只跳过坏的那行/那个文件并写进报告，其余照常导入；坏文件原样进归档，不丢。（Task 7、8 的测试钉住）
2. 时间戳带小数秒、带时区偏移（`+08:00`）→ 正确换算成 UTC 毫秒格式；解析不了的保留原串并计数，不丢行。（Task 6）
3. 客户端发来一行不是 JSON → 回一个 `id: 0` 的错误响应，连接不断，后续请求照常处理。（Task 9）
4. 第二个 crewd 用同一个数据目录启动 → 立刻以非 0 退出并说明原因；上次崩溃留下的 socket 文件不挡新实例启动。（Task 9、10）
5. 中文、emoji 文本经 post → 存储 → list 原样往返。（Task 4、10）

---

### Task 1: Workspace 骨架 + proto crate

**Files:**
- Create: `crewd/Cargo.toml`, `crewd/rust-toolchain.toml`, `crewd/scripts/check.sh`
- Create: `crewd/crates/proto/Cargo.toml`, `crewd/crates/proto/src/lib.rs`, `crewd/crates/proto/src/model.rs`
- Modify: `.gitignore`（加 `crewd/target/`）

**Interfaces:**
- Produces（crate `crewd_proto`）:
  - `pub const API_VERSION: u32 = 1;`
  - `Request { id: u64, method: String, params: Value }`
  - `RpcError { code: i32, message: String }`，常量 `PARSE=-32700, METHOD_NOT_FOUND=-32601, INVALID_PARAMS=-32602, INTERNAL=-32603`，`RpcError::new(code, msg)`
  - `Response { id, result: Option<Value>, error: Option<RpcError> }`，`Response::ok(id, Value)`、`Response::err(id, RpcError)`
  - `Event { event: String, data: Value }`；`enum ServerFrame { Response(Response), Event(Event) }`（untagged）
  - `pub fn parse_params<T: DeserializeOwned>(Value) -> Result<T, RpcError>`
  - `mod methods`：`HELLO="system.hello"`、`CREW_LIST="crew.list"`、`WHITEBOARD_LIST="whiteboard.list"`、`WHITEBOARD_POST="whiteboard.post"`、`TODO_LIST="todo.list"`、`SUBSCRIBE="events.subscribe"`
  - `mod events`：`WHITEBOARD_APPENDED="whiteboard.appended"`、`LAGGED="events.lagged"`
  - model：`Hello`、`Crew`、`Session`、`Mention`、`Attachment`、`Message`、`Ledger`（`Agent|Human`，`as_str()`）、`Todo`、`TodoResponse`、`WhiteboardListParams`、`WhiteboardPostParams`、`TodoListParams`、`SubscribeParams`
  - `pub fn api_schema() -> Value`

- [ ] **Step 1: 建 workspace**

```bash
mkdir -p crewd/crates crewd/scripts
cd crewd
cat > Cargo.toml <<'EOF'
[workspace]
resolver = "3"
members = ["crates/*"]

[workspace.package]
edition = "2024"
version = "0.1.0"
license = "MIT"
EOF
cat > rust-toolchain.toml <<'EOF'
[toolchain]
channel = "1.94.0"
components = ["rustfmt", "clippy"]
EOF
cat > scripts/check.sh <<'EOF'
#!/bin/sh
# crewd 的唯一检查入口：格式、lint、测试。任何一步失败即非 0 退出。
set -eu
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
EOF
chmod +x scripts/check.sh
cargo new --lib crates/proto --name crewd-proto --vcs none
```

把 `crates/proto/Cargo.toml` 的 `[package]` 里 `edition = ...` 那行换成 `edition.workspace = true`，并加 `version.workspace = true`、`license.workspace = true`（删掉原来的 `version` 行）。之后每个新 crate 都做同样替换。

在仓库根 `.gitignore` 末尾追加：

```
# crewd 构建产物
crewd/target/
```

- [ ] **Step 2: 加依赖**

```bash
cargo add -p crewd-proto serde --features derive
cargo add -p crewd-proto serde_json schemars
```

- [ ] **Step 3: 写失败的测试**

`crates/proto/src/lib.rs` 末尾（实现之前先放测试，`cargo test` 会因为类型不存在而编译失败）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_params_default_to_null() {
        let r: Request = serde_json::from_str(r#"{"id":7,"method":"system.hello"}"#).unwrap();
        assert_eq!(r.id, 7);
        assert_eq!(r.params, Value::Null);
    }

    #[test]
    fn server_frame_distinguishes_response_and_event() {
        let f: ServerFrame = serde_json::from_str(r#"{"id":1,"result":{"ok":true}}"#).unwrap();
        assert!(matches!(f, ServerFrame::Response(_)));
        let f: ServerFrame = serde_json::from_str(r#"{"event":"whiteboard.appended","data":{}}"#).unwrap();
        assert!(matches!(f, ServerFrame::Event(_)));
    }

    #[test]
    fn response_omits_absent_fields() {
        let s = serde_json::to_value(Response::ok(3, json!(1))).unwrap();
        assert_eq!(s, json!({"id":3,"result":1}));
    }

    #[test]
    fn message_uses_camel_case_and_skips_none() {
        let m = Message {
            seq: 5,
            id: "m1".into(),
            crew_id: "local-a".into(),
            sender_kind: "user".into(),
            sender_session_id: None,
            sender_name: None,
            text: "你好 👋".into(),
            category: None,
            in_reply_to: Some("m0".into()),
            mentions: vec![Mention { kind: "session".into(), target_id: Some("worker-1".into()) }],
            attachments: vec![],
            created_at: "2026-10-05T00:00:00.000Z".into(),
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["crewId"], "local-a");
        assert_eq!(v["inReplyTo"], "m0");
        assert_eq!(v["mentions"][0]["targetId"], "worker-1");
        assert!(v.get("senderName").is_none());
    }

    #[test]
    fn parse_params_reports_invalid_params() {
        let e = parse_params::<WhiteboardListParams>(json!({"limit": 3})).unwrap_err();
        assert_eq!(e.code, RpcError::INVALID_PARAMS);
    }

    #[test]
    fn ledger_serializes_lowercase() {
        assert_eq!(serde_json::to_value(Ledger::Human).unwrap(), json!("human"));
        assert_eq!(Ledger::Agent.as_str(), "agent");
    }

    #[test]
    fn api_schema_lists_methods_and_types() {
        let s = api_schema();
        assert_eq!(s["apiVersion"], 1);
        assert!(s["methods"]["whiteboard.post"].is_object());
        assert!(s["types"]["Message"].is_object());
    }
}
```

- [ ] **Step 4: 跑测试确认失败**

Run: `cargo test -p crewd-proto`
Expected: 编译失败，`cannot find type Request`。

- [ ] **Step 5: 实现**

`crates/proto/src/model.rs`：

```rust
//! 领域模型：客户端看到的机组、session、消息、Todo。字段名即线上 JSON（camelCase）。
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub server: String,
    pub version: String,
    pub api_version: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Crew {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<i64>,
    pub builtin: bool,
    pub parent_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captain_agent_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub crew_id: String,
    /// "captain" | "worker"
    pub role: String,
    /// "claude_code" | "codex" | "unknown"
    pub agent_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Mention {
    /// "session" | "captain" | "human" | "broadcast"
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: String,
    pub mime: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// 全库单调递增的序号；客户端用它增量拉取。
    pub seq: i64,
    pub id: String,
    pub crew_id: String,
    /// "user" | "session" | "captain" | "pendingcrew" | 旧数据里的 "human"
    pub sender_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_name: Option<String>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub mentions: Vec<Mention>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Ledger {
    Agent,
    Human,
}

impl Ledger {
    pub fn as_str(self) -> &'static str {
        match self {
            Ledger::Agent => "agent",
            Ledger::Human => "human",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TodoResponse {
    pub id: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_name: Option<String>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Todo {
    pub id: String,
    pub crew_id: String,
    pub ledger: Ledger,
    /// 账内编号 #N；软删除的行也占号。
    pub number: i64,
    pub text: String,
    /// "pending" | "in_progress" | "completed" | "blocked_on_human" | "dropped"
    pub status: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub responses: Vec<TodoResponse>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WhiteboardListParams {
    pub crew_id: String,
    /// 只返回 seq 大于它的消息；缺省 0。
    #[serde(default)]
    pub after_seq: Option<i64>,
    /// 缺省 200，上限 1000。
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WhiteboardPostParams {
    pub crew_id: String,
    pub text: String,
    pub sender_kind: String,
    #[serde(default)]
    pub sender_session_id: Option<String>,
    #[serde(default)]
    pub sender_name: Option<String>,
    #[serde(default)]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub mentions: Vec<Mention>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TodoListParams {
    pub crew_id: String,
    pub ledger: Ledger,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeParams {
    /// 主题 = 事件名第一个点之前的部分，例如 "whiteboard"。
    pub topics: Vec<String>,
}
```

`crates/proto/src/lib.rs`（放在 Step 3 的测试模块之前）：

```rust
//! crewd 线协议：客户端（Mac / iOS / CLI）与 crewd 之间的唯一契约。
//! 传输：每行一个 JSON（NDJSON）。客户端发 `Request`，服务端回 `ServerFrame`。
mod model;

pub use model::*;
use schemars::{JsonSchema, schema_for};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const API_VERSION: u32 = 1;

pub mod methods {
    pub const HELLO: &str = "system.hello";
    pub const CREW_LIST: &str = "crew.list";
    pub const WHITEBOARD_LIST: &str = "whiteboard.list";
    pub const WHITEBOARD_POST: &str = "whiteboard.post";
    pub const TODO_LIST: &str = "todo.list";
    pub const SUBSCRIBE: &str = "events.subscribe";
}

pub mod events {
    pub const WHITEBOARD_APPENDED: &str = "whiteboard.appended";
    /// 订阅者处理太慢、漏掉了事件；`data.missed` 是漏掉的条数，客户端应全量重拉。
    pub const LAGGED: &str = "events.lagged";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

impl RpcError {
    pub const PARSE: i32 = -32700;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL: i32 = -32603;

    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Response {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn ok(id: u64, result: Value) -> Self {
        Self { id, result: Some(result), error: None }
    }
    pub fn err(id: u64, error: RpcError) -> Self {
        Self { id, result: None, error: Some(error) }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Event {
    pub event: String,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ServerFrame {
    Response(Response),
    Event(Event),
}

pub fn parse_params<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params)
        .map_err(|e| RpcError::new(RpcError::INVALID_PARAMS, format!("参数无效: {e}")))
}

/// 整份 API 契约。`crewd schema` 打印它；仓库里提交一份，测试保证两者一致。
pub fn api_schema() -> Value {
    json!({
        "apiVersion": API_VERSION,
        "transport": "NDJSON over Unix socket; one Request per line, server replies ServerFrame per line",
        "methods": {
            methods::HELLO: { "params": null, "result": "Hello" },
            methods::CREW_LIST: { "params": null, "result": "Crew[]" },
            methods::WHITEBOARD_LIST: { "params": "WhiteboardListParams", "result": "Message[]" },
            methods::WHITEBOARD_POST: { "params": "WhiteboardPostParams", "result": "Message" },
            methods::TODO_LIST: { "params": "TodoListParams", "result": "Todo[]" },
            methods::SUBSCRIBE: { "params": "SubscribeParams", "result": "{ topics: string[] }" },
        },
        "events": {
            events::WHITEBOARD_APPENDED: "Message",
            events::LAGGED: "{ missed: number }",
        },
        "types": {
            "Request": schema_for!(Request),
            "Response": schema_for!(Response),
            "Event": schema_for!(Event),
            "Hello": schema_for!(Hello),
            "Crew": schema_for!(Crew),
            "Session": schema_for!(Session),
            "Message": schema_for!(Message),
            "Todo": schema_for!(Todo),
            "WhiteboardListParams": schema_for!(WhiteboardListParams),
            "WhiteboardPostParams": schema_for!(WhiteboardPostParams),
            "TodoListParams": schema_for!(TodoListParams),
            "SubscribeParams": schema_for!(SubscribeParams),
        }
    })
}
```

- [ ] **Step 6: 跑测试确认通过**

Run: `cargo test -p crewd-proto`
Expected: 7 passed。

- [ ] **Step 7: 跑全量检查并提交**

Run: `./scripts/check.sh`（失败就按 clippy/fmt 提示修，`cargo fmt --all` 可自动格式化）
Expected: 退出码 0。

```bash
cd ..
git add .gitignore crewd/Cargo.toml crewd/Cargo.lock crewd/rust-toolchain.toml crewd/scripts/check.sh crewd/crates/proto
git commit -m "crewd: workspace skeleton and wire protocol types

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: store —— 打开、迁移、meta、机组与 session

**Files:**
- Create: `crewd/crates/store/Cargo.toml`, `src/lib.rs`, `src/schema.rs`, `src/crews.rs`, `src/sessions.rs`

**Interfaces:**
- Consumes: `crewd_proto::{Crew, Session}`
- Produces（crate `crewd_store`）:
  - `pub type Result<T> = std::result::Result<T, StoreError>;` `enum StoreError { Sql(rusqlite::Error), Json(serde_json::Error), Invalid(String) }`
  - `pub struct Store`：`Store::open(&Path) -> Result<Store>`、`Store::open_in_memory() -> Result<Store>`、`fn conn(&self) -> &rusqlite::Connection`、`fn with_tx<T>(&mut self, f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>) -> Result<T>`、`fn is_empty(&self) -> Result<bool>`
  - `pub fn meta_get(&Connection, &str) -> Result<Option<String>>`、`pub fn meta_set(&Connection, &str, &str) -> Result<()>`
  - `pub fn now_ts() -> String`（UTC 毫秒 RFC3339）
  - `pub fn is_unique_violation(&rusqlite::Error) -> bool`
  - `crews::upsert(&Connection, &Crew, Option<&Value>) -> Result<()>`、`crews::set_parents(&Connection, &str, &[String]) -> Result<()>`、`crews::exists(&Connection, &str) -> Result<bool>`、`crews::list(&Connection) -> Result<Vec<Crew>>`
  - `sessions::upsert(&Connection, &Session, Option<&Value>) -> Result<()>`、`sessions::list_for_crew(&Connection, &str) -> Result<Vec<Session>>`
  - `schema::SCHEMA_VERSION: i64 = 1`

- [ ] **Step 1: 建 crate、加依赖**

```bash
cd crewd
cargo new --lib crates/store --name crewd-store --vcs none
# 按 Task 1 Step 1 的说明把 Cargo.toml 改成 workspace 继承
cargo add -p crewd-store rusqlite --features bundled
cargo add -p crewd-store serde_json thiserror chrono
cargo add -p crewd-store crewd-proto --path crates/proto
```

- [ ] **Step 2: 写失败的测试**

`crates/store/src/lib.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crewd_proto::{Crew, Session};
    use serde_json::json;

    fn crew(id: &str, n: Option<i64>) -> Crew {
        Crew {
            id: id.into(),
            title: format!("机组 {id}"),
            number: n,
            builtin: false,
            parent_ids: vec![],
            working_directory: Some("/tmp/w".into()),
            captain_agent_kind: Some("claude_code".into()),
            hidden_at: None,
            created_at: "2026-10-01T00:00:00.000Z".into(),
            updated_at: "2026-10-01T00:00:00.000Z".into(),
        }
    }

    #[test]
    fn fresh_store_is_empty_and_versioned() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.is_empty().unwrap());
        let v: i64 = s.conn().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, schema::SCHEMA_VERSION);
    }

    #[test]
    fn reopening_file_store_keeps_data() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("crewd.db");
        {
            let s = Store::open(&p).unwrap();
            crews::upsert(s.conn(), &crew("a", Some(1)), None).unwrap();
        }
        let s = Store::open(&p).unwrap();
        assert_eq!(crews::list(s.conn()).unwrap().len(), 1);
        assert!(!s.is_empty().unwrap());
    }

    #[test]
    fn newer_schema_version_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("crewd.db");
        {
            let c = rusqlite::Connection::open(&p).unwrap();
            c.execute_batch("PRAGMA user_version = 99").unwrap();
        }
        let err = Store::open(&p).err().unwrap();
        assert!(err.to_string().contains("99"));
    }

    #[test]
    fn meta_roundtrip() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(meta_get(s.conn(), "k").unwrap(), None);
        meta_set(s.conn(), "k", "v1").unwrap();
        meta_set(s.conn(), "k", "v2").unwrap();
        assert_eq!(meta_get(s.conn(), "k").unwrap().as_deref(), Some("v2"));
    }

    #[test]
    fn crews_list_includes_parents_and_extra_is_kept() {
        let s = Store::open_in_memory().unwrap();
        crews::upsert(s.conn(), &crew("a", Some(2)), Some(&json!({"runtimeLocation":"local_host"}))).unwrap();
        crews::upsert(s.conn(), &crew("b", Some(1)), None).unwrap();
        crews::set_parents(s.conn(), "b", &["a".to_string()]).unwrap();
        let list = crews::list(s.conn()).unwrap();
        assert_eq!(list.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        assert_eq!(list[0].parent_ids, vec!["a".to_string()]);
        let extra: String = s.conn().query_row("SELECT extra FROM crews WHERE id='a'", [], |r| r.get(0)).unwrap();
        assert!(extra.contains("local_host"));
        assert!(crews::exists(s.conn(), "a").unwrap());
        assert!(!crews::exists(s.conn(), "zz").unwrap());
    }

    #[test]
    fn with_tx_rolls_back_on_error() {
        let mut s = Store::open_in_memory().unwrap();
        let r: Result<()> = s.with_tx(|tx| {
            crews::upsert(tx, &crew("a", None), None)?;
            Err(StoreError::Invalid("boom".into()))
        });
        assert!(r.is_err());
        assert!(s.is_empty().unwrap());
    }

    #[test]
    fn sessions_upsert_and_list() {
        let s = Store::open_in_memory().unwrap();
        crews::upsert(s.conn(), &crew("a", None), None).unwrap();
        let mut ses = Session {
            id: "captain-1".into(),
            crew_id: "a".into(),
            role: "captain".into(),
            agent_kind: "claude_code".into(),
            agent_session_id: Some("uuid-1".into()),
            display_name: Some("机长".into()),
            working_directory: None,
            model: None,
            effort: None,
            updated_at: "2026-10-01T00:00:00.000Z".into(),
        };
        sessions::upsert(s.conn(), &ses, None).unwrap();
        ses.display_name = Some("新机长".into());
        sessions::upsert(s.conn(), &ses, None).unwrap();
        let list = sessions::list_for_crew(s.conn(), "a").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].display_name.as_deref(), Some("新机长"));
    }

    #[test]
    fn now_ts_is_utc_millis() {
        let t = now_ts();
        assert_eq!(t.len(), "2026-10-05T00:00:00.000Z".len());
        assert!(t.ends_with('Z'));
    }
}
```

```bash
cargo add -p crewd-store --dev tempfile
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p crewd-store`
Expected: 编译失败（`Store` 等未定义）。

- [ ] **Step 4: 实现**

`crates/store/src/schema.rs`：

```rust
//! 表结构与迁移。版本号存在 SQLite 的 `PRAGMA user_version`。
use crate::{Result, StoreError};
use rusqlite::Connection;

pub const SCHEMA_VERSION: i64 = 1;

const V1: &str = r#"
BEGIN;
CREATE TABLE meta(
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE crews(
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  number INTEGER,
  builtin INTEGER NOT NULL DEFAULT 0,
  working_directory TEXT,
  captain_agent_kind TEXT,
  hidden_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  extra TEXT
);
CREATE TABLE crew_parents(
  crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
  parent_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
  PRIMARY KEY(crew_id, parent_id)
);
CREATE TABLE sessions(
  id TEXT PRIMARY KEY,
  crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
  role TEXT NOT NULL,
  agent_kind TEXT NOT NULL,
  agent_session_id TEXT,
  display_name TEXT,
  working_directory TEXT,
  model TEXT,
  effort TEXT,
  updated_at TEXT NOT NULL,
  extra TEXT
);
CREATE TABLE messages(
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT NOT NULL UNIQUE,
  crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
  sender_kind TEXT NOT NULL,
  sender_session_id TEXT,
  sender_name TEXT,
  text TEXT NOT NULL,
  category TEXT,
  in_reply_to TEXT,
  mentions TEXT NOT NULL DEFAULT '[]',
  attachments TEXT NOT NULL DEFAULT '[]',
  created_at TEXT NOT NULL,
  extra TEXT
);
CREATE INDEX messages_crew_seq ON messages(crew_id, seq);
CREATE TABLE todos(
  id TEXT PRIMARY KEY,
  crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
  ledger TEXT NOT NULL CHECK(ledger IN ('agent','human')),
  number INTEGER NOT NULL,
  text TEXT NOT NULL,
  status TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT,
  deleted_at TEXT,
  attachments TEXT NOT NULL DEFAULT '[]',
  extra TEXT,
  UNIQUE(crew_id, ledger, number)
);
CREATE TABLE todo_responses(
  id TEXT PRIMARY KEY,
  todo_id TEXT NOT NULL REFERENCES todos(id) ON DELETE CASCADE,
  ord INTEGER NOT NULL,
  session_id TEXT NOT NULL,
  sender_name TEXT,
  text TEXT NOT NULL,
  status TEXT,
  attachments TEXT NOT NULL DEFAULT '[]',
  created_at TEXT NOT NULL
);
CREATE TABLE legacy_archive(
  path TEXT PRIMARY KEY,
  content BLOB NOT NULL
);
PRAGMA user_version = 1;
COMMIT;
"#;

pub fn migrate(conn: &Connection) -> Result<()> {
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if v > SCHEMA_VERSION {
        return Err(StoreError::Invalid(format!(
            "数据库版本 {v} 比本程序支持的 {SCHEMA_VERSION} 新，拒绝打开"
        )));
    }
    if v < 1 {
        conn.execute_batch(V1)?;
    }
    Ok(())
}
```

`crates/store/src/lib.rs`（测试模块之前）：

```rust
//! crewd 的持久化层。crewd 是数据库的唯一写者；其他进程一律经 crewd 的 API。
pub mod crews;
pub mod schema;
pub mod sessions;

use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        // journal_mode 会返回一行结果，所以用 query_row 而不是 execute。
        let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        schema::migrate(&conn)?;
        Ok(Self { conn })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn with_tx<T>(
        &mut self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let tx = self.conn.transaction()?;
        let v = f(&tx)?;
        tx.commit()?;
        Ok(v)
    }

    /// 没有任何机组、也没有任何消息。导入只往空库里写。
    pub fn is_empty(&self) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT (SELECT COUNT(*) FROM crews) + (SELECT COUNT(*) FROM messages)",
            [],
            |r| r.get(0),
        )?;
        Ok(n == 0)
    }
}

pub fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
        .optional()?)
}

pub fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn now_ts() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn is_unique_violation(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation)
}

/// 把一列 JSON 文本解成类型；失败转成 rusqlite 的列转换错误，便于在 query_map 里用 `?`。
pub(crate) fn json_col<T: serde::de::DeserializeOwned>(
    row: &rusqlite::Row<'_>,
    idx: usize,
) -> rusqlite::Result<T> {
    let s: String = row.get(idx)?;
    serde_json::from_str(&s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, Box::new(e))
    })
}
```

`json_col` 用到 `serde`：`cargo add -p crewd-store serde`。

`crates/store/src/crews.rs`：

```rust
use crate::Result;
use crewd_proto::Crew;
use rusqlite::{Connection, params};
use serde_json::Value;

pub fn upsert(conn: &Connection, c: &Crew, extra: Option<&Value>) -> Result<()> {
    conn.execute(
        "INSERT INTO crews(id,title,number,builtin,working_directory,captain_agent_kind,hidden_at,created_at,updated_at,extra)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
         ON CONFLICT(id) DO UPDATE SET title=excluded.title, number=excluded.number, builtin=excluded.builtin,
           working_directory=excluded.working_directory, captain_agent_kind=excluded.captain_agent_kind,
           hidden_at=excluded.hidden_at, updated_at=excluded.updated_at, extra=excluded.extra",
        params![
            c.id,
            c.title,
            c.number,
            c.builtin,
            c.working_directory,
            c.captain_agent_kind,
            c.hidden_at,
            c.created_at,
            c.updated_at,
            extra.map(|v| v.to_string()),
        ],
    )?;
    Ok(())
}

pub fn set_parents(conn: &Connection, crew_id: &str, parents: &[String]) -> Result<()> {
    conn.execute("DELETE FROM crew_parents WHERE crew_id=?1", [crew_id])?;
    for p in parents {
        conn.execute(
            "INSERT INTO crew_parents(crew_id,parent_id) VALUES(?1,?2)",
            params![crew_id, p],
        )?;
    }
    Ok(())
}

pub fn exists(conn: &Connection, id: &str) -> Result<bool> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM crews WHERE id=?1", [id], |r| r.get(0))?;
    Ok(n > 0)
}

/// 按编号升序（无编号的排最后），同号按创建时间。
pub fn list(conn: &Connection) -> Result<Vec<Crew>> {
    let mut stmt = conn.prepare(
        "SELECT id,title,number,builtin,working_directory,captain_agent_kind,hidden_at,created_at,updated_at
         FROM crews ORDER BY number IS NULL, number, created_at",
    )?;
    let mut crews = stmt
        .query_map([], |r| {
            Ok(Crew {
                id: r.get(0)?,
                title: r.get(1)?,
                number: r.get(2)?,
                builtin: r.get(3)?,
                parent_ids: vec![],
                working_directory: r.get(4)?,
                captain_agent_kind: r.get(5)?,
                hidden_at: r.get(6)?,
                created_at: r.get(7)?,
                updated_at: r.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut pstmt =
        conn.prepare("SELECT parent_id FROM crew_parents WHERE crew_id=?1 ORDER BY parent_id")?;
    for c in &mut crews {
        c.parent_ids = pstmt
            .query_map([&c.id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
    }
    Ok(crews)
}
```

`crates/store/src/sessions.rs`：

```rust
use crate::Result;
use crewd_proto::Session;
use rusqlite::{Connection, params};
use serde_json::Value;

pub fn upsert(conn: &Connection, s: &Session, extra: Option<&Value>) -> Result<()> {
    conn.execute(
        "INSERT INTO sessions(id,crew_id,role,agent_kind,agent_session_id,display_name,working_directory,model,effort,updated_at,extra)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
         ON CONFLICT(id) DO UPDATE SET crew_id=excluded.crew_id, role=excluded.role, agent_kind=excluded.agent_kind,
           agent_session_id=excluded.agent_session_id, display_name=excluded.display_name,
           working_directory=excluded.working_directory, model=excluded.model, effort=excluded.effort,
           updated_at=excluded.updated_at, extra=excluded.extra",
        params![
            s.id,
            s.crew_id,
            s.role,
            s.agent_kind,
            s.agent_session_id,
            s.display_name,
            s.working_directory,
            s.model,
            s.effort,
            s.updated_at,
            extra.map(|v| v.to_string()),
        ],
    )?;
    Ok(())
}

pub fn list_for_crew(conn: &Connection, crew_id: &str) -> Result<Vec<Session>> {
    let mut stmt = conn.prepare(
        "SELECT id,crew_id,role,agent_kind,agent_session_id,display_name,working_directory,model,effort,updated_at
         FROM sessions WHERE crew_id=?1 ORDER BY id",
    )?;
    let rows = stmt
        .query_map([crew_id], |r| {
            Ok(Session {
                id: r.get(0)?,
                crew_id: r.get(1)?,
                role: r.get(2)?,
                agent_kind: r.get(3)?,
                agent_session_id: r.get(4)?,
                display_name: r.get(5)?,
                working_directory: r.get(6)?,
                model: r.get(7)?,
                effort: r.get(8)?,
                updated_at: r.get(9)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd-store`
Expected: 8 passed。

- [ ] **Step 6: 检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
cd ..
git add crewd/Cargo.lock crewd/crates/store
git commit -m "crewd: SQLite store with schema v1, crews and sessions

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: store —— 消息、Todo、归档

**Files:**
- Create: `crewd/crates/store/src/messages.rs`, `src/todos.rs`, `src/archive.rs`
- Create: `crewd/crates/store/tests/content.rs`
- Modify: `crewd/crates/store/src/lib.rs`（加 `pub mod messages; pub mod todos; pub mod archive;`）

**Interfaces:**
- Consumes: Task 2 的 `Store`、`json_col`、`Result`；`crewd_proto::{Message, Todo, TodoResponse, Ledger}`
- Produces:
  - `messages::insert(&Connection, &Message, Option<&Value>) -> Result<Option<i64>>`（id 已存在返回 `None`，否则返回新 seq；`Message.seq` 入参被忽略）
  - `messages::list(&Connection, crew_id: &str, after_seq: i64, limit: u32) -> Result<Vec<Message>>`（seq 升序）
  - `todos::insert(&Connection, &Todo, Option<&Value>) -> Result<()>`（连同 responses，按顺序写 `ord`）
  - `todos::list(&Connection, crew_id: &str, ledger: Ledger) -> Result<Vec<Todo>>`（按 number 升序，含软删除行）
  - `archive::put(&Connection, path: &str, content: &[u8]) -> Result<()>`、`archive::get(&Connection, &str) -> Result<Option<Vec<u8>>>`、`archive::list_paths(&Connection) -> Result<Vec<String>>`

- [ ] **Step 1: 写失败的测试**

`crates/store/tests/content.rs`：

```rust
use crewd_proto::{Attachment, Crew, Ledger, Mention, Message, Todo, TodoResponse};
use crewd_store::{Store, archive, crews, messages, todos};

fn store_with_crew() -> Store {
    let s = Store::open_in_memory().unwrap();
    crews::upsert(
        s.conn(),
        &Crew {
            id: "a".into(),
            title: "A".into(),
            number: None,
            builtin: false,
            parent_ids: vec![],
            working_directory: None,
            captain_agent_kind: None,
            hidden_at: None,
            created_at: "2026-10-01T00:00:00.000Z".into(),
            updated_at: "2026-10-01T00:00:00.000Z".into(),
        },
        None,
    )
    .unwrap();
    s
}

fn msg(id: &str, text: &str) -> Message {
    Message {
        seq: 0,
        id: id.into(),
        crew_id: "a".into(),
        sender_kind: "user".into(),
        sender_session_id: None,
        sender_name: Some("我".into()),
        text: text.into(),
        category: None,
        in_reply_to: None,
        mentions: vec![Mention { kind: "broadcast".into(), target_id: None }],
        attachments: vec![Attachment {
            id: "f1".into(),
            mime: "text/plain".into(),
            size: Some(3),
            path: "/x/a.txt".into(),
            filename: None,
        }],
        created_at: "2026-10-01T00:00:00.000Z".into(),
    }
}

#[test]
fn messages_insert_assigns_increasing_seq_and_ignores_duplicates() {
    let s = store_with_crew();
    let a = messages::insert(s.conn(), &msg("m1", "一"), None).unwrap().unwrap();
    let b = messages::insert(s.conn(), &msg("m2", "二"), None).unwrap().unwrap();
    assert!(b > a);
    assert_eq!(messages::insert(s.conn(), &msg("m1", "重复"), None).unwrap(), None);
}

#[test]
fn messages_list_after_seq_with_limit_roundtrips_unicode() {
    let s = store_with_crew();
    let text = "中文 + emoji 👋🏽 + 换行\n第二行";
    let first = messages::insert(s.conn(), &msg("m1", text), None).unwrap().unwrap();
    messages::insert(s.conn(), &msg("m2", "二"), None).unwrap();
    messages::insert(s.conn(), &msg("m3", "三"), None).unwrap();
    let all = messages::list(s.conn(), "a", 0, 10).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].text, text);
    assert_eq!(all[0].seq, first);
    assert_eq!(all[0].mentions[0].kind, "broadcast");
    assert_eq!(all[0].attachments[0].size, Some(3));
    let tail = messages::list(s.conn(), "a", first, 1).unwrap();
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].id, "m2");
    assert!(messages::list(s.conn(), "other", 0, 10).unwrap().is_empty());
}

#[test]
fn todos_insert_and_list_with_responses_in_order() {
    let s = store_with_crew();
    let t = Todo {
        id: "t1".into(),
        crew_id: "a".into(),
        ledger: Ledger::Agent,
        number: 2,
        text: "发版".into(),
        status: "in_progress".into(),
        created_at: "2026-10-01T00:00:00.000Z".into(),
        updated_at: None,
        deleted_at: None,
        attachments: vec![],
        responses: vec![
            TodoResponse {
                id: "r1".into(),
                session_id: "captain-1".into(),
                sender_name: None,
                text: "收到".into(),
                status: Some("in_progress".into()),
                attachments: vec![],
                created_at: "2026-10-01T00:01:00.000Z".into(),
            },
            TodoResponse {
                id: "r2".into(),
                session_id: "worker-1".into(),
                sender_name: Some("w".into()),
                text: "做到一半".into(),
                status: None,
                attachments: vec![],
                created_at: "2026-10-01T00:02:00.000Z".into(),
            },
        ],
    };
    let mut earlier = t.clone();
    earlier.id = "t0".into();
    earlier.number = 1;
    earlier.responses.clear();
    earlier.deleted_at = Some("2026-10-02T00:00:00.000Z".into());
    todos::insert(s.conn(), &t, None).unwrap();
    todos::insert(s.conn(), &earlier, None).unwrap();
    let list = todos::list(s.conn(), "a", Ledger::Agent).unwrap();
    assert_eq!(list.iter().map(|t| t.number).collect::<Vec<_>>(), [1, 2]);
    assert!(list[0].deleted_at.is_some());
    assert_eq!(list[1].responses.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["r1", "r2"]);
    assert!(todos::list(s.conn(), "a", Ledger::Human).unwrap().is_empty());
}

#[test]
fn todos_duplicate_number_in_same_ledger_is_unique_violation() {
    let s = store_with_crew();
    let mk = |id: &str| Todo {
        id: id.into(),
        crew_id: "a".into(),
        ledger: Ledger::Human,
        number: 1,
        text: "x".into(),
        status: "pending".into(),
        created_at: "2026-10-01T00:00:00.000Z".into(),
        updated_at: None,
        deleted_at: None,
        attachments: vec![],
        responses: vec![],
    };
    todos::insert(s.conn(), &mk("t1"), None).unwrap();
    let err = todos::insert(s.conn(), &mk("t2"), None).unwrap_err();
    match err {
        crewd_store::StoreError::Sql(e) => assert!(crewd_store::is_unique_violation(&e)),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn archive_roundtrip() {
    let s = store_with_crew();
    archive::put(s.conn(), "whiteboards/a.plan.json", b"[1]").unwrap();
    assert_eq!(archive::get(s.conn(), "whiteboards/a.plan.json").unwrap().unwrap(), b"[1]");
    assert_eq!(archive::get(s.conn(), "nope").unwrap(), None);
    assert_eq!(archive::list_paths(s.conn()).unwrap(), ["whiteboards/a.plan.json"]);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-store --test content`
Expected: 编译失败（`messages` 模块不存在）。

- [ ] **Step 3: 实现**

`crates/store/src/messages.rs`：

```rust
use crate::{Result, json_col};
use crewd_proto::Message;
use rusqlite::{Connection, params};
use serde_json::Value;

/// 写入一条消息。id 已存在 → `Ok(None)`（幂等）；否则返回分配到的 seq。
pub fn insert(conn: &Connection, m: &Message, extra: Option<&Value>) -> Result<Option<i64>> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO messages(id,crew_id,sender_kind,sender_session_id,sender_name,text,category,in_reply_to,mentions,attachments,created_at,extra)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            m.id,
            m.crew_id,
            m.sender_kind,
            m.sender_session_id,
            m.sender_name,
            m.text,
            m.category,
            m.in_reply_to,
            serde_json::to_string(&m.mentions)?,
            serde_json::to_string(&m.attachments)?,
            m.created_at,
            extra.map(|v| v.to_string()),
        ],
    )?;
    Ok(if n == 0 { None } else { Some(conn.last_insert_rowid()) })
}

pub fn list(conn: &Connection, crew_id: &str, after_seq: i64, limit: u32) -> Result<Vec<Message>> {
    let mut stmt = conn.prepare(
        "SELECT seq,id,crew_id,sender_kind,sender_session_id,sender_name,text,category,in_reply_to,mentions,attachments,created_at
         FROM messages WHERE crew_id=?1 AND seq>?2 ORDER BY seq LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(params![crew_id, after_seq, limit], |r| {
            Ok(Message {
                seq: r.get(0)?,
                id: r.get(1)?,
                crew_id: r.get(2)?,
                sender_kind: r.get(3)?,
                sender_session_id: r.get(4)?,
                sender_name: r.get(5)?,
                text: r.get(6)?,
                category: r.get(7)?,
                in_reply_to: r.get(8)?,
                mentions: json_col(r, 9)?,
                attachments: json_col(r, 10)?,
                created_at: r.get(11)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
```

`crates/store/src/todos.rs`：

```rust
use crate::{Result, json_col};
use crewd_proto::{Ledger, Todo, TodoResponse};
use rusqlite::{Connection, params};
use serde_json::Value;

pub fn insert(conn: &Connection, t: &Todo, extra: Option<&Value>) -> Result<()> {
    conn.execute(
        "INSERT INTO todos(id,crew_id,ledger,number,text,status,created_at,updated_at,deleted_at,attachments,extra)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            t.id,
            t.crew_id,
            t.ledger.as_str(),
            t.number,
            t.text,
            t.status,
            t.created_at,
            t.updated_at,
            t.deleted_at,
            serde_json::to_string(&t.attachments)?,
            extra.map(|v| v.to_string()),
        ],
    )?;
    for (ord, r) in t.responses.iter().enumerate() {
        conn.execute(
            "INSERT INTO todo_responses(id,todo_id,ord,session_id,sender_name,text,status,attachments,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                r.id,
                t.id,
                ord as i64,
                r.session_id,
                r.sender_name,
                r.text,
                r.status,
                serde_json::to_string(&r.attachments)?,
                r.created_at,
            ],
        )?;
    }
    Ok(())
}

pub fn list(conn: &Connection, crew_id: &str, ledger: Ledger) -> Result<Vec<Todo>> {
    let mut stmt = conn.prepare(
        "SELECT id,crew_id,number,text,status,created_at,updated_at,deleted_at,attachments
         FROM todos WHERE crew_id=?1 AND ledger=?2 ORDER BY number",
    )?;
    let mut todos = stmt
        .query_map(params![crew_id, ledger.as_str()], |r| {
            Ok(Todo {
                id: r.get(0)?,
                crew_id: r.get(1)?,
                ledger,
                number: r.get(2)?,
                text: r.get(3)?,
                status: r.get(4)?,
                created_at: r.get(5)?,
                updated_at: r.get(6)?,
                deleted_at: r.get(7)?,
                attachments: json_col(r, 8)?,
                responses: vec![],
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut rstmt = conn.prepare(
        "SELECT id,session_id,sender_name,text,status,attachments,created_at
         FROM todo_responses WHERE todo_id=?1 ORDER BY ord",
    )?;
    for t in &mut todos {
        t.responses = rstmt
            .query_map([&t.id], |r| {
                Ok(TodoResponse {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    sender_name: r.get(2)?,
                    text: r.get(3)?,
                    status: r.get(4)?,
                    attachments: json_col(r, 5)?,
                    created_at: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
    }
    Ok(todos)
}
```

`crates/store/src/archive.rs`：

```rust
//! 旧数据里本里程碑不建模的文件（计划板、sweep、续跑凭证、侧边栏排序……）原样存档，
//! 后续里程碑（M4 迁移计划板等）从这里读。
use crate::Result;
use rusqlite::{Connection, OptionalExtension, params};

pub fn put(conn: &Connection, path: &str, content: &[u8]) -> Result<()> {
    conn.execute(
        "INSERT INTO legacy_archive(path,content) VALUES(?1,?2)
         ON CONFLICT(path) DO UPDATE SET content=excluded.content",
        params![path, content],
    )?;
    Ok(())
}

pub fn get(conn: &Connection, path: &str) -> Result<Option<Vec<u8>>> {
    Ok(conn
        .query_row("SELECT content FROM legacy_archive WHERE path=?1", [path], |r| r.get(0))
        .optional()?)
}

pub fn list_paths(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT path FROM legacy_archive ORDER BY path")?;
    let v = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(v)
}
```

在 `lib.rs` 顶部模块声明处加：

```rust
pub mod archive;
pub mod messages;
pub mod todos;
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-store`
Expected: 单元 8 + 集成 5 全过。

- [ ] **Step 5: 检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
cd ..
git add crewd/crates/store
git commit -m "crewd: store messages, todos and legacy archive

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: legacy crate 骨架 —— 路径、报告、宽容读行、时间戳

**Files:**
- Create: `crewd/crates/legacy/Cargo.toml`, `src/lib.rs`, `src/paths.rs`, `src/report.rs`, `src/rows.rs`, `src/model.rs`

**Interfaces:**
- Consumes: `crewd_proto::Attachment`
- Produces（crate `crewd_legacy`，本任务只放出内部件，`import()` 在 Task 8 组装）:
  - `pub mod paths`：`default_legacy_root() -> PathBuf`、`struct PathRewrite { from: PathBuf, to: PathBuf }` + `apply(&self, &str) -> String`、`copy_dir(&Path, &Path) -> std::io::Result<usize>`
  - `pub struct ImportReport { counts: BTreeMap<String, usize>, skipped: Vec<Skipped>, warnings: Vec<String> }`，方法 `add(&mut self, &str, usize)`、`bump(&mut self, &str)`、`skip(&mut self, root: &Path, file: &Path, index: Option<usize>, reason: impl Into<String>)`、`warn(&mut self, impl Into<String>)`；`pub struct Skipped { file: String, index: Option<usize>, reason: String }`；都 `Serialize`
  - `pub(crate) struct Ctx<'a> { src: &'a Path, report: &'a mut ImportReport, consumed: &'a mut BTreeSet<PathBuf>, rewrite: &'a PathRewrite }`
  - `rows::read_array(&mut Ctx, &Path) -> Option<Vec<Value>>`（成功读到数组才把文件记为 consumed）
  - `rows::read_rows<T: DeserializeOwned>(&mut Ctx, &Path) -> Vec<T>`
  - `rows::norm_ts(&mut Ctx, &str) -> String`
  - `rows::conv_att(&mut Ctx, model::LAttachment) -> Attachment`
  - `model::{LCrew, LMember, LAgentSession, LMention, LAttachment, LMessage, LTodo, LTodoResponse}`（字段见下）

- [ ] **Step 1: 建 crate、加依赖**

```bash
cd crewd
cargo new --lib crates/legacy --name crewd-legacy --vcs none
# 改 Cargo.toml 为 workspace 继承
cargo add -p crewd-legacy serde --features derive
cargo add -p crewd-legacy serde_json chrono dirs anyhow
cargo add -p crewd-legacy rusqlite --features bundled
cargo add -p crewd-legacy crewd-proto --path crates/proto
cargo add -p crewd-legacy crewd-store --path crates/store
cargo add -p crewd-legacy --dev tempfile
```

（`rusqlite` 只为拿到 `Connection` 类型；features 与 store 一致。）

- [ ] **Step 2: 写失败的测试**

`crates/legacy/src/lib.rs`（先只有模块声明 + 测试）：

```rust
//! 把旧 Swift 版的 JSON 数据只读地导入 crewd 的 SQLite。源目录一个字节都不改。
pub mod paths;
mod model;
mod report;
mod rows;

pub use report::{ImportReport, Skipped};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub(crate) struct Ctx<'a> {
    pub src: &'a Path,
    pub report: &'a mut ImportReport,
    pub consumed: &'a mut BTreeSet<PathBuf>,
    pub rewrite: &'a paths::PathRewrite,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn ctx_parts(dir: &Path) -> (ImportReport, BTreeSet<PathBuf>, paths::PathRewrite) {
        (
            ImportReport::default(),
            BTreeSet::new(),
            paths::PathRewrite { from: dir.join("attachments"), to: PathBuf::from("/new/attachments") },
        )
    }

    #[test]
    fn norm_ts_converts_offsets_and_keeps_unparseable() {
        let dir = tempfile::tempdir().unwrap();
        let (mut r, mut c, rw) = ctx_parts(dir.path());
        let mut cx = Ctx { src: dir.path(), report: &mut r, consumed: &mut c, rewrite: &rw };
        assert_eq!(rows::norm_ts(&mut cx, "2026-09-11T12:34:56Z"), "2026-09-11T12:34:56.000Z");
        assert_eq!(rows::norm_ts(&mut cx, "2026-09-11T12:34:56.789+08:00"), "2026-09-11T04:34:56.789Z");
        assert_eq!(rows::norm_ts(&mut cx, "昨天"), "昨天");
        assert_eq!(r.counts.get("timestampsUnparsed"), Some(&1));
    }

    #[test]
    fn read_rows_skips_bad_rows_and_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.json");
        fs::write(&good, r#"[{"id":"a","mime":"x","path":"/p"},{"id":"b"},{"id":"c","mime":"y","path":"/q"}]"#).unwrap();
        let bad = dir.path().join("bad.json");
        fs::write(&bad, "{not json").unwrap();
        let obj = dir.path().join("obj.json");
        fs::write(&obj, r#"{"a":1}"#).unwrap();
        let (mut r, mut c, rw) = ctx_parts(dir.path());
        let mut cx = Ctx { src: dir.path(), report: &mut r, consumed: &mut c, rewrite: &rw };
        let rows: Vec<model::LAttachment> = rows::read_rows(&mut cx, &good);
        assert_eq!(rows.len(), 2);
        assert!(rows::read_rows::<model::LAttachment>(&mut cx, &bad).is_empty());
        assert!(rows::read_rows::<model::LAttachment>(&mut cx, &obj).is_empty());
        assert!(rows::read_rows::<model::LAttachment>(&mut cx, &dir.path().join("missing.json")).is_empty());
        assert_eq!(r.skipped.len(), 3);
        assert_eq!(r.skipped[0].file, "good.json");
        assert_eq!(r.skipped[0].index, Some(1));
        assert!(c.contains(&good));
        assert!(!c.contains(&bad), "坏文件不算 consumed，留给归档原样保存");
    }

    #[test]
    fn path_rewrite_only_touches_paths_under_source() {
        let rw = paths::PathRewrite { from: "/old/attachments".into(), to: "/new/attachments".into() };
        assert_eq!(rw.apply("/old/attachments/c1/a.png"), "/new/attachments/c1/a.png");
        assert_eq!(rw.apply("/elsewhere/a.png"), "/elsewhere/a.png");
    }

    #[test]
    fn copy_dir_copies_recursively() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("a");
        fs::create_dir_all(from.join("c1")).unwrap();
        fs::write(from.join("c1/x.txt"), "x").unwrap();
        fs::write(from.join("y.txt"), "y").unwrap();
        let to = dir.path().join("b");
        assert_eq!(paths::copy_dir(&from, &to).unwrap(), 2);
        assert_eq!(fs::read_to_string(to.join("c1/x.txt")).unwrap(), "x");
        assert_eq!(paths::copy_dir(&dir.path().join("nope"), &to).unwrap(), 0);
    }

    #[test]
    fn legacy_message_keeps_unknown_fields_in_extra() {
        let m: model::LMessage = serde_json::from_str(
            r#"{"id":"m","senderKind":"user","text":"t","createdAt":"2026-09-11T00:00:00Z",
                "mentions":[{"kind":"session","target_id":"worker-1"}],"audience":"human","headline":"h"}"#,
        )
        .unwrap();
        assert_eq!(m.mentions[0].target_id.as_deref(), Some("worker-1"));
        assert_eq!(m.extra["audience"], "human");
        assert!(!m.extra.contains_key("text"));
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p crewd-legacy`
Expected: 编译失败（`paths`、`rows` 等模块文件不存在）。

- [ ] **Step 4: 实现**

`crates/legacy/src/paths.rs`：

```rust
use std::fs;
use std::path::{Path, PathBuf};

/// 旧版数据根：`PENDINGCREW_DATA_DIR`（trim、展开 `~/`）> `~/Library/Application Support/PendingCrew`。
pub fn default_legacy_root() -> PathBuf {
    if let Ok(v) = std::env::var("PENDINGCREW_DATA_DIR") {
        let v = v.trim();
        if !v.is_empty() {
            return expand_tilde(v);
        }
    }
    dirs::data_dir().expect("无法确定用户数据目录").join("PendingCrew")
}

fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(s)
}

/// 旧数据里附件存的是绝对路径；导入时把源目录下的附件前缀换成新数据目录。
pub struct PathRewrite {
    pub from: PathBuf,
    pub to: PathBuf,
}

impl PathRewrite {
    pub fn apply(&self, p: &str) -> String {
        match Path::new(p).strip_prefix(&self.from) {
            Ok(rest) => self.to.join(rest).to_string_lossy().into_owned(),
            Err(_) => p.to_string(),
        }
    }
}

/// 递归复制，返回复制的文件数。源不存在返回 0。
pub fn copy_dir(from: &Path, to: &Path) -> std::io::Result<usize> {
    if !from.is_dir() {
        return Ok(0);
    }
    fs::create_dir_all(to)?;
    let mut n = 0;
    for e in fs::read_dir(from)? {
        let e = e?;
        let p = e.path();
        let t = to.join(e.file_name());
        if p.is_dir() {
            n += copy_dir(&p, &t)?;
        } else {
            fs::copy(&p, &t)?;
            n += 1;
        }
    }
    Ok(n)
}
```

`crates/legacy/src/report.rs`：

```rust
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub counts: BTreeMap<String, usize>,
    pub skipped: Vec<Skipped>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Skipped {
    /// 相对源目录的路径。
    pub file: String,
    /// 数组里的第几行；`None` 表示整个文件。
    pub index: Option<usize>,
    pub reason: String,
}

impl ImportReport {
    pub fn add(&mut self, key: &str, n: usize) {
        *self.counts.entry(key.to_string()).or_default() += n;
    }
    pub fn bump(&mut self, key: &str) {
        self.add(key, 1);
    }
    pub fn skip(&mut self, root: &Path, file: &Path, index: Option<usize>, reason: impl Into<String>) {
        self.skipped.push(Skipped {
            file: file.strip_prefix(root).unwrap_or(file).display().to_string(),
            index,
            reason: reason.into(),
        });
    }
    pub fn warn(&mut self, w: impl Into<String>) {
        self.warnings.push(w.into());
    }
}
```

`crates/legacy/src/model.rs`：

```rust
//! 旧 Swift Codable 的 JSON 形状（只读）。键名 = Swift 属性名（camelCase），
//! 例外：mention 的 `target_id`。未知键全部落进 `extra`，导入时原样存进对应行的 extra 列。
use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LCrew {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub is_builtin: bool,
    pub crew_number: Option<i64>,
    pub working_directory: Option<String>,
    pub captain_agent_kind: Option<String>,
    #[serde(default)]
    pub parent_crew_ids: Vec<String>,
    #[serde(default)]
    pub session_members: Vec<LMember>,
    pub manually_hidden_at: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LMember {
    pub session_id: String,
    pub display_name: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LAgentSession {
    pub crew_id: String,
    pub session_id: String,
    pub kind: String,
    pub agent_session_id: String,
    pub updated_at: String,
    pub working_directory: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
pub struct LMention {
    pub kind: String,
    pub target_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LAttachment {
    pub id: String,
    pub mime: String,
    pub size: Option<i64>,
    pub path: String,
    pub filename: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LMessage {
    pub id: String,
    pub sender_kind: String,
    pub text: String,
    pub created_at: String,
    pub sender_session_id: Option<String>,
    pub sender_name: Option<String>,
    pub category: Option<String>,
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub mentions: Vec<LMention>,
    #[serde(default)]
    pub attachments: Vec<LAttachment>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LTodoResponse {
    pub id: String,
    pub session_id: String,
    pub text: String,
    pub created_at: String,
    pub sender_name: Option<String>,
    pub status: Option<String>,
    #[serde(default)]
    pub attachments: Vec<LAttachment>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LTodo {
    pub id: String,
    pub number: i64,
    pub text: String,
    pub status: String,
    pub created_at: String,
    #[serde(default)]
    pub responses: Vec<LTodoResponse>,
    pub updated_at: Option<String>,
    pub deleted_at: Option<String>,
    #[serde(default)]
    pub attachments: Vec<LAttachment>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
```

`crates/legacy/src/rows.rs`：

```rust
use crate::Ctx;
use crate::model::LAttachment;
use chrono::{DateTime, SecondsFormat, Utc};
use crewd_proto::Attachment;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::Path;

/// 读一个「顶层是数组」的账本文件。不存在 → None（不报）；读不出/不是 JSON/不是数组 →
/// None 并记一条整文件跳过（不标 consumed，归档会原样保存它）。
pub fn read_array(cx: &mut Ctx, path: &Path) -> Option<Vec<Value>> {
    if !path.exists() {
        return None;
    }
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            cx.report.skip(cx.src, path, None, format!("读取失败: {e}"));
            return None;
        }
    };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Array(rows)) => {
            cx.consumed.insert(path.to_path_buf());
            Some(rows)
        }
        Ok(_) => {
            cx.report.skip(cx.src, path, None, "顶层不是数组");
            None
        }
        Err(e) => {
            cx.report.skip(cx.src, path, None, format!("JSON 无效: {e}"));
            None
        }
    }
}

/// 逐行解码；坏行记跳过（带下标），其余照收。
pub fn read_rows<T: DeserializeOwned>(cx: &mut Ctx, path: &Path) -> Vec<T> {
    let Some(values) = read_array(cx, path) else {
        return vec![];
    };
    let mut out = Vec::with_capacity(values.len());
    for (i, v) in values.into_iter().enumerate() {
        match serde_json::from_value::<T>(v) {
            Ok(t) => out.push(t),
            Err(e) => cx.report.skip(cx.src, path, Some(i), e.to_string()),
        }
    }
    out
}

/// 统一成 UTC 毫秒 RFC3339。解析不了就原样保留并计数——宁可排序略乱，也不丢行。
pub fn norm_ts(cx: &mut Ctx, raw: &str) -> String {
    match DateTime::parse_from_rfc3339(raw.trim()) {
        Ok(t) => t.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::Millis, true),
        Err(_) => {
            cx.report.bump("timestampsUnparsed");
            raw.to_string()
        }
    }
}

pub fn conv_att(cx: &mut Ctx, a: LAttachment) -> Attachment {
    let path = cx.rewrite.apply(&a.path);
    if path == a.path {
        cx.report.bump("attachmentPathsNotRewritten");
    }
    Attachment { id: a.id, mime: a.mime, size: a.size, path, filename: a.filename }
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd-legacy`
Expected: 5 passed。（`model` 里暂未用到的类型可能触发 dead_code 警告，clippy 在 Task 8 用上后消失；本任务提交前若 `check.sh` 因 dead_code 失败，在 `mod model;` 和 `mod rows;` 前临时加 `#[allow(dead_code)]`，Task 8 Step 4 删掉。）

- [ ] **Step 6: 检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
cd ..
git add crewd/Cargo.lock crewd/crates/legacy
git commit -m "crewd: legacy import scaffolding (paths, report, lenient rows, timestamps)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 5: 合成的旧数据夹具

**Files:**
- Create: `crewd/crates/legacy/tests/fixtures/basic/` 下全部文件（见下）

**Interfaces:**
- Produces: 夹具目录 `crates/legacy/tests/fixtures/basic`，Task 8 与 Task 10 的测试都用它。其预期导入结果（Task 8 断言）：
  - crews 3（`pendingcrew-chief`、`local-aaaa`、`local-bbbb`；bbbb 的父 = aaaa；aaaa 有一条指向不存在机组的父边 → `danglingParentEdges` 1）
  - sessions 3（2 条有记录 + 1 条只在成员表里 → `sessionsWithoutRecord` 1）
  - messages 3（aaaa 板 2 条有效 + outbox 1 条；1 坏行跳过；1 条 read-failure 合成行丢弃）
  - agentTodos 2、humanTodos 1、todoResponses 1
  - `whiteboards/local-bbbb.json` 是坏 JSON → 整文件跳过并进归档
  - 归档含 `crew-arrangement.json`、`whiteboards/local-aaaa.plan.json`、`whiteboards/local-bbbb.json`；不含 `whiteboards/crew-sessions.json`、`whiteboards/local-aaaa.captain-1111aaaa.cursor`
  - attachmentFiles 1

- [ ] **Step 1: 写夹具文件**

```bash
F=crewd/crates/legacy/tests/fixtures/basic
mkdir -p $F/whiteboards/outbox $F/attachments/local-aaaa
cat > $F/local-crews.json <<'EOF'
{
  "version": 1,
  "nextCrewNumber": 3,
  "crews": [
    {"id":"pendingcrew-chief","title":"总机组","responsibleSubjectId":"s0","runtimeLocation":"local_host",
     "createdAt":"2026-08-01T00:00:00Z","updatedAt":"2026-08-01T00:00:00Z","isBuiltin":true},
    {"id":"local-aaaa","title":"示例机组","responsibleSubjectId":"s1","runtimeLocation":"local_host",
     "createdAt":"2026-09-01T08:00:00Z","updatedAt":"2026-09-02T08:00:00Z","crewNumber":1,
     "workingDirectory":"/tmp/example","captainAgentKind":"claude_code","titleSource":"human",
     "parentCrewIds":["local-gone"],
     "sessionMembers":[
       {"sessionId":"captain-1111aaaa","displayName":"机长","createdAt":"2026-09-01T08:00:00Z"},
       {"sessionId":"worker-2222bbbb","displayName":"滚动修复","createdAt":"2026-09-01T09:00:00Z","extensionNumber":2},
       {"sessionId":"worker-3333cccc","displayName":"旧成员","createdAt":"2026-09-01T10:00:00Z"}
     ]},
    {"id":"local-bbbb","title":"子机组","responsibleSubjectId":"s2","runtimeLocation":"local_host",
     "createdAt":"2026-09-03T08:00:00Z","updatedAt":"2026-09-03T08:00:00Z","crewNumber":2,
     "parentCrewIds":["local-aaaa"]},
    {"title":"缺 id 的坏行"}
  ]
}
EOF
cat > $F/whiteboards/agent-sessions.json <<'EOF'
[
  {"crewId":"local-aaaa","sessionId":"captain-1111aaaa","kind":"claude_code",
   "agentSessionId":"00000000-0000-4000-8000-000000000001","updatedAt":"2026-09-01T08:00:00Z","structuredPost":true},
  {"crewId":"local-aaaa","sessionId":"worker-2222bbbb","kind":"codex","agentSessionId":"thread-xyz",
   "updatedAt":"2026-09-01T09:00:00Z","model":"gpt-5.6","effort":"high","workingDirectory":"/tmp/example"},
  {"crewId":"local-aaaa","sessionId":"worker-2222bbbb","kind":"codex","agentSessionId":"thread-old",
   "updatedAt":"2026-08-01T09:00:00Z"}
]
EOF
cat > $F/whiteboards/local-aaaa.json <<'EOF'
[
  {"id":"11111111-0000-4000-8000-000000000001","senderKind":"user","senderUserId":"local-byok-user",
   "text":"你好，大家 👋","createdAt":"2026-09-11T12:34:56Z",
   "mentions":[{"kind":"session","target_id":"worker-2222bbbb"}],"audience":"agents"},
  {"id":"11111111-0000-4000-8000-000000000002","senderKind":"session","senderSessionId":"worker-2222bbbb",
   "senderName":"滚动修复","text":"收到","createdAt":"2026-09-11T12:35:00.250+08:00",
   "inReplyTo":"11111111-0000-4000-8000-000000000001","category":"ack",
   "attachments":[{"id":"att1","mime":"text/plain","size":5,"path":"/nonexistent/attachments/local-aaaa/note.txt","filename":"note.txt"}]},
  {"id":"11111111-0000-4000-8000-000000000003","senderKind":"user","createdAt":"2026-09-11T12:36:00Z"},
  {"id":"whiteboard-read-failure","senderKind":"pendingcrew","text":"读不出","createdAt":"2026-09-11T12:37:00Z"}
]
EOF
cat > $F/whiteboards/outbox/local-aaaa.1757594300000.1.11111111-0000-4000-8000-000000000004.json <<'EOF'
{"id":"11111111-0000-4000-8000-000000000004","senderKind":"captain","senderSessionId":"captain-1111aaaa",
 "text":"还没合并进白板的一条","createdAt":"2026-09-11T12:38:00Z"}
EOF
printf '{not json' > $F/whiteboards/local-bbbb.json
cat > $F/whiteboards/local-aaaa.todos.json <<'EOF'
[
  {"id":"22222222-0000-4000-8000-000000000001","number":1,"text":"已删的一条","status":"dropped",
   "createdAt":"2026-09-05T00:00:00Z","responses":[],"deletedAt":"2026-09-06T00:00:00Z"},
  {"id":"22222222-0000-4000-8000-000000000002","number":2,"text":"发一个新版","status":"in_progress",
   "createdAt":"2026-09-07T00:00:00Z","updatedAt":"2026-09-07T01:00:00Z",
   "responses":[{"id":"33333333-0000-4000-8000-000000000001","sessionId":"captain-1111aaaa",
     "text":"我来","status":"in_progress","createdAt":"2026-09-07T00:10:00Z"}]}
]
EOF
cat > $F/whiteboards/local-aaaa.human-todos.json <<'EOF'
[
  {"id":"44444444-0000-4000-8000-000000000001","number":1,"text":"要不要开 IP 日志？","status":"pending",
   "createdAt":"2026-09-08T00:00:00Z","responses":[],"createdBySessionId":"worker-2222bbbb",
   "createdBySenderName":"滚动修复","resumeNote":"回复后继续"}
]
EOF
cat > $F/whiteboards/local-aaaa.plan.json <<'EOF'
[{"id":"55555555-0000-4000-8000-000000000001","number":1,"title":"计划一","status":"in_progress",
  "updates":[],"createdAt":"2026-09-09T00:00:00Z","updatedAt":"2026-09-09T00:00:00Z"}]
EOF
cat > $F/whiteboards/crew-sessions.json <<'EOF'
{"crews":{},"updatedAt":"2026-09-11T00:00:00Z"}
EOF
printf '11111111-0000-4000-8000-000000000001\t2026-09-11T12:34:56Z' > $F/whiteboards/local-aaaa.captain-1111aaaa.cursor
cat > $F/crew-arrangement.json <<'EOF'
{"crewIds":["local-bbbb","local-aaaa"],"reason":"示例","createdAt":"2026-09-10T00:00:00Z"}
EOF
printf 'hello' > $F/attachments/local-aaaa/note.txt
```

- [ ] **Step 2: 确认文件齐全**

Run: `find crewd/crates/legacy/tests/fixtures/basic -type f | sort`
Expected: 恰好 12 个文件（local-crews.json、crew-arrangement.json、attachments/local-aaaa/note.txt、whiteboards 下 9 个）。

- [ ] **Step 3: 提交**

```bash
git add crewd/crates/legacy/tests/fixtures/basic
git commit -m "crewd: synthetic legacy data fixture

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 6: 导入机组与 session

**Files:**
- Create: `crewd/crates/legacy/src/crews.rs`, `src/sessions.rs`
- Modify: `crewd/crates/legacy/src/lib.rs`（加 `mod crews; mod sessions;`）

**Interfaces:**
- Consumes: Task 4 的 `Ctx`、`rows::*`、`model::*`；`crewd_store::{crews, sessions, Result}`
- Produces:
  - `crews::ImportedCrew { id: String, members: Vec<LMember> }`
  - `crews::import(&rusqlite::Connection, &mut Ctx) -> crewd_store::Result<Vec<ImportedCrew>>`
  - `sessions::import(&rusqlite::Connection, &mut Ctx, &[ImportedCrew]) -> crewd_store::Result<()>`

- [ ] **Step 1: 写失败的测试**

`crates/legacy/src/lib.rs` 的 `mod tests` 里追加：

```rust
    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/basic")
    }

    #[test]
    fn crews_and_sessions_import_from_fixture() {
        let src = fixture();
        let store = crewd_store::Store::open_in_memory().unwrap();
        let (mut r, mut c, rw) = ctx_parts(&src);
        let mut cx = Ctx { src: &src, report: &mut r, consumed: &mut c, rewrite: &rw };
        let crews = crews::import(store.conn(), &mut cx).unwrap();
        sessions::import(store.conn(), &mut cx, &crews).unwrap();

        let list = crewd_store::crews::list(store.conn()).unwrap();
        let ids: Vec<_> = list.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["local-aaaa", "local-bbbb", "pendingcrew-chief"]);
        let b = list.iter().find(|c| c.id == "local-bbbb").unwrap();
        assert_eq!(b.parent_ids, ["local-aaaa"]);
        let chief = list.iter().find(|c| c.id == "pendingcrew-chief").unwrap();
        assert!(chief.builtin);
        assert_eq!(list[0].created_at, "2026-09-01T08:00:00.000Z");

        let ses = crewd_store::sessions::list_for_crew(store.conn(), "local-aaaa").unwrap();
        assert_eq!(ses.len(), 3);
        let cap = ses.iter().find(|s| s.id == "captain-1111aaaa").unwrap();
        assert_eq!(cap.role, "captain");
        assert_eq!(cap.display_name.as_deref(), Some("机长"));
        let w = ses.iter().find(|s| s.id == "worker-2222bbbb").unwrap();
        assert_eq!(w.agent_session_id.as_deref(), Some("thread-xyz"), "取 updatedAt 最新的那条");
        assert_eq!(w.model.as_deref(), Some("gpt-5.6"));
        let old = ses.iter().find(|s| s.id == "worker-3333cccc").unwrap();
        assert_eq!(old.agent_kind, "unknown");

        assert_eq!(r.counts["crews"], 3);
        assert_eq!(r.counts["sessions"], 3);
        assert_eq!(r.counts["sessionsWithoutRecord"], 1);
        assert_eq!(r.counts["danglingParentEdges"], 1);
        assert!(r.skipped.iter().any(|s| s.file == "local-crews.json" && s.index == Some(3)));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-legacy crews_and_sessions`
Expected: 编译失败（`crews` 模块不存在）。

- [ ] **Step 3: 实现**

`crates/legacy/src/crews.rs`：

```rust
use crate::Ctx;
use crate::model::{LCrew, LMember};
use crate::rows::norm_ts;
use crewd_proto::Crew;
use rusqlite::Connection;
use serde_json::Value;

pub struct ImportedCrew {
    pub id: String,
    pub members: Vec<LMember>,
}

/// `local-crews.json`：旧版要求整份可解码；这里逐行宽容，坏行跳过。
pub fn import(tx: &Connection, cx: &mut Ctx) -> crewd_store::Result<Vec<ImportedCrew>> {
    let path = cx.src.join("local-crews.json");
    if !path.exists() {
        cx.report.warn("源目录里没有 local-crews.json，没有机组可导入");
        return Ok(vec![]);
    }
    let file: Value = match std::fs::read(&path)
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
    {
        Ok(v) => v,
        Err(e) => {
            cx.report.skip(cx.src, &path, None, e);
            return Ok(vec![]);
        }
    };
    cx.consumed.insert(path.clone());
    let rows = file.get("crews").and_then(Value::as_array).cloned().unwrap_or_default();

    let mut crews = Vec::new();
    let mut parents: Vec<(String, Vec<String>)> = Vec::new();
    for (i, v) in rows.into_iter().enumerate() {
        let c: LCrew = match serde_json::from_value(v) {
            Ok(c) => c,
            Err(e) => {
                cx.report.skip(cx.src, &path, Some(i), e.to_string());
                continue;
            }
        };
        let hidden_at = c.manually_hidden_at.as_deref().map(|t| norm_ts(cx, t));
        let crew = Crew {
            id: c.id.clone(),
            title: c.title.clone(),
            number: c.crew_number,
            builtin: c.is_builtin,
            parent_ids: vec![],
            working_directory: c.working_directory.clone(),
            captain_agent_kind: c.captain_agent_kind.clone(),
            hidden_at,
            created_at: norm_ts(cx, &c.created_at),
            updated_at: norm_ts(cx, &c.updated_at),
        };
        crewd_store::crews::upsert(tx, &crew, Some(&Value::Object(c.extra)))?;
        cx.report.bump("crews");
        parents.push((c.id.clone(), c.parent_crew_ids));
        crews.push(ImportedCrew { id: c.id, members: c.session_members });
    }

    for (id, ps) in parents {
        let mut known = Vec::new();
        for p in ps {
            if crews.iter().any(|c| c.id == p) {
                known.push(p);
            } else {
                cx.report.bump("danglingParentEdges");
            }
        }
        crewd_store::crews::set_parents(tx, &id, &known)?;
    }
    Ok(crews)
}
```

`crates/legacy/src/sessions.rs`：

```rust
use crate::Ctx;
use crate::crews::ImportedCrew;
use crate::model::LAgentSession;
use crate::rows::{norm_ts, read_rows};
use crewd_proto::Session;
use rusqlite::Connection;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

fn role_of(session_id: &str) -> String {
    if session_id.starts_with("captain-") { "captain" } else { "worker" }.to_string()
}

/// session = `agent-sessions.json` 的记录（同 id 取 updatedAt 最新）+ 机组成员表里的显示名；
/// 只在成员表里出现、没有记录的，以 agent_kind="unknown" 入库。
pub fn import(tx: &Connection, cx: &mut Ctx, crews: &[ImportedCrew]) -> crewd_store::Result<()> {
    let path = cx.src.join("whiteboards").join("agent-sessions.json");
    let recs: Vec<LAgentSession> = read_rows(cx, &path);

    let mut by_id: BTreeMap<String, LAgentSession> = BTreeMap::new();
    for r in recs {
        let keep_old = by_id
            .get(&r.session_id)
            .is_some_and(|old| old.updated_at >= r.updated_at);
        if keep_old {
            cx.report.bump("sessionRecordsSuperseded");
        } else {
            if by_id.contains_key(&r.session_id) {
                cx.report.bump("sessionRecordsSuperseded");
            }
            by_id.insert(r.session_id.clone(), r);
        }
    }

    let names: BTreeMap<String, String> = crews
        .iter()
        .flat_map(|c| c.members.iter().map(|m| (m.session_id.clone(), m.display_name.clone())))
        .collect();
    let mut seen = BTreeSet::new();

    for r in by_id.into_values() {
        if !crews.iter().any(|c| c.id == r.crew_id) {
            cx.report.bump("sessionsForUnknownCrew");
            continue;
        }
        let s = Session {
            id: r.session_id.clone(),
            crew_id: r.crew_id.clone(),
            role: role_of(&r.session_id),
            agent_kind: r.kind.clone(),
            agent_session_id: Some(r.agent_session_id.clone()),
            display_name: names.get(&r.session_id).cloned(),
            working_directory: r.working_directory.clone(),
            model: r.model.clone(),
            effort: r.effort.clone(),
            updated_at: norm_ts(cx, &r.updated_at),
        };
        crewd_store::sessions::upsert(tx, &s, Some(&Value::Object(r.extra)))?;
        cx.report.bump("sessions");
        seen.insert(r.session_id);
    }

    for c in crews {
        for m in &c.members {
            if !seen.insert(m.session_id.clone()) {
                continue;
            }
            let s = Session {
                id: m.session_id.clone(),
                crew_id: c.id.clone(),
                role: role_of(&m.session_id),
                agent_kind: "unknown".into(),
                agent_session_id: None,
                display_name: Some(m.display_name.clone()),
                working_directory: None,
                model: None,
                effort: None,
                updated_at: norm_ts(cx, &m.created_at),
            };
            crewd_store::sessions::upsert(tx, &s, None)?;
            cx.report.bump("sessions");
            cx.report.bump("sessionsWithoutRecord");
        }
    }
    Ok(())
}
```

在 `lib.rs` 加 `mod crews; mod sessions;`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-legacy`
Expected: 6 passed。

- [ ] **Step 5: 检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
cd ..
git add crewd/crates/legacy/src
git commit -m "crewd: import legacy crews and sessions

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 7: 导入白板消息、Todo 与归档

**Files:**
- Create: `crewd/crates/legacy/src/whiteboard.rs`, `src/todos.rs`, `src/archive.rs`
- Modify: `crewd/crates/legacy/src/lib.rs`（加 `mod whiteboard; mod todos; mod archive;`）

**Interfaces:**
- Consumes: Task 4/6；`crewd_store::{messages, todos, archive, crews, is_unique_violation, StoreError}`
- Produces:
  - `whiteboard::import_board(&Connection, &mut Ctx, crew_id: &str) -> crewd_store::Result<()>`
  - `whiteboard::import_outbox(&Connection, &mut Ctx) -> crewd_store::Result<()>`
  - `todos::import(&Connection, &mut Ctx, crew_id: &str, Ledger) -> crewd_store::Result<()>`
  - `archive::import_rest(&Connection, &mut Ctx) -> crewd_store::Result<()>`

- [ ] **Step 1: 写失败的测试**

`lib.rs` 的 `mod tests` 追加：

```rust
    fn import_parts(src: &Path, store: &crewd_store::Store) -> ImportReport {
        let (mut r, mut c, rw) = ctx_parts(src);
        let mut cx = Ctx { src, report: &mut r, consumed: &mut c, rewrite: &rw };
        let crews = crews::import(store.conn(), &mut cx).unwrap();
        sessions::import(store.conn(), &mut cx, &crews).unwrap();
        for c in &crews {
            whiteboard::import_board(store.conn(), &mut cx, &c.id).unwrap();
            todos::import(store.conn(), &mut cx, &c.id, crewd_proto::Ledger::Agent).unwrap();
            todos::import(store.conn(), &mut cx, &c.id, crewd_proto::Ledger::Human).unwrap();
        }
        whiteboard::import_outbox(store.conn(), &mut cx).unwrap();
        archive::import_rest(store.conn(), &mut cx).unwrap();
        r
    }

    #[test]
    fn messages_todos_and_archive_import_from_fixture() {
        let src = fixture();
        let store = crewd_store::Store::open_in_memory().unwrap();
        let r = import_parts(&src, &store);

        let msgs = crewd_store::messages::list(store.conn(), "local-aaaa", 0, 100).unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].text, "你好，大家 👋");
        assert_eq!(msgs[0].mentions[0].target_id.as_deref(), Some("worker-2222bbbb"));
        assert_eq!(msgs[1].created_at, "2026-09-11T04:35:00.250Z");
        assert_eq!(msgs[1].in_reply_to.as_deref(), Some("11111111-0000-4000-8000-000000000001"));
        assert_eq!(msgs[2].sender_kind, "captain", "outbox 里的那条排在板上消息之后");
        let extra: String = store
            .conn()
            .query_row("SELECT extra FROM messages WHERE id='11111111-0000-4000-8000-000000000001'", [], |x| x.get(0))
            .unwrap();
        assert!(extra.contains("\"audience\":\"agents\""));

        let agent = crewd_store::todos::list(store.conn(), "local-aaaa", crewd_proto::Ledger::Agent).unwrap();
        assert_eq!(agent.len(), 2);
        assert!(agent[0].deleted_at.is_some());
        assert_eq!(agent[1].responses.len(), 1);
        let human = crewd_store::todos::list(store.conn(), "local-aaaa", crewd_proto::Ledger::Human).unwrap();
        assert_eq!(human.len(), 1);

        let paths = crewd_store::archive::list_paths(store.conn()).unwrap();
        assert!(paths.contains(&"crew-arrangement.json".to_string()));
        assert!(paths.contains(&"whiteboards/local-aaaa.plan.json".to_string()));
        assert!(paths.contains(&"whiteboards/local-bbbb.json".to_string()), "坏文件原样归档");
        assert!(!paths.iter().any(|p| p.ends_with("crew-sessions.json")));
        assert!(!paths.iter().any(|p| p.ends_with(".cursor")));
        assert!(!paths.iter().any(|p| p.starts_with("attachments/")));
        assert!(!paths.iter().any(|p| p == "whiteboards/local-aaaa.json"), "已导入的不重复归档");

        assert_eq!(r.counts["messages"], 3);
        assert_eq!(r.counts["outboxMessages"], 1);
        assert_eq!(r.counts["syntheticRowsDropped"], 1);
        assert_eq!(r.counts["agentTodos"], 2);
        assert_eq!(r.counts["humanTodos"], 1);
        assert_eq!(r.counts["todoResponses"], 1);
        assert_eq!(r.counts["attachmentPathsNotRewritten"], 1);
        assert!(r.skipped.iter().any(|s| s.file == "whiteboards/local-aaaa.json" && s.index == Some(2)));
        assert!(r.skipped.iter().any(|s| s.file == "whiteboards/local-bbbb.json" && s.index.is_none()));
    }

    #[test]
    fn importing_same_board_twice_is_idempotent_for_messages() {
        let src = fixture();
        let store = crewd_store::Store::open_in_memory().unwrap();
        let (mut r, mut c, rw) = ctx_parts(&src);
        let mut cx = Ctx { src: &src, report: &mut r, consumed: &mut c, rewrite: &rw };
        crews::import(store.conn(), &mut cx).unwrap();
        whiteboard::import_board(store.conn(), &mut cx, "local-aaaa").unwrap();
        whiteboard::import_board(store.conn(), &mut cx, "local-aaaa").unwrap();
        assert_eq!(crewd_store::messages::list(store.conn(), "local-aaaa", 0, 100).unwrap().len(), 2);
        assert_eq!(r.counts["duplicateMessages"], 2);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-legacy`
Expected: 编译失败（`whiteboard` 模块不存在）。

- [ ] **Step 3: 实现**

`crates/legacy/src/whiteboard.rs`：

```rust
use crate::Ctx;
use crate::model::LMessage;
use crate::rows::{conv_att, norm_ts, read_rows};
use crewd_proto::{Mention, Message};
use rusqlite::Connection;
use serde_json::Value;
use std::path::PathBuf;

/// 旧版白板读失败时会重建一行内存警示，不是真实消息。
const READ_FAILURE_ID: &str = "whiteboard-read-failure";

pub fn import_board(tx: &Connection, cx: &mut Ctx, crew_id: &str) -> crewd_store::Result<()> {
    let path = cx.src.join("whiteboards").join(format!("{crew_id}.json"));
    let rows: Vec<LMessage> = read_rows(cx, &path);
    for m in rows {
        insert_one(tx, cx, crew_id, m)?;
    }
    Ok(())
}

/// `whiteboards/outbox/<crewId>.<ts>.<seq>.<uuid>.json`：还没合并进白板的写入。按文件名排序导入。
pub fn import_outbox(tx: &Connection, cx: &mut Ctx) -> crewd_store::Result<()> {
    let dir = cx.src.join("whiteboards").join("outbox");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Ok(());
    };
    let mut files: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    for f in files {
        let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let crew_id = name.split('.').next().unwrap_or_default().to_string();
        let m: LMessage = match std::fs::read(&f)
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
        {
            Ok(m) => m,
            Err(e) => {
                cx.report.skip(cx.src, &f, None, e);
                continue;
            }
        };
        if !crewd_store::crews::exists(tx, &crew_id)? {
            cx.report.skip(cx.src, &f, None, format!("所属机组 {crew_id} 不存在"));
            continue;
        }
        cx.consumed.insert(f.clone());
        cx.report.bump("outboxMessages");
        insert_one(tx, cx, &crew_id, m)?;
    }
    Ok(())
}

fn insert_one(tx: &Connection, cx: &mut Ctx, crew_id: &str, m: LMessage) -> crewd_store::Result<()> {
    if m.id == READ_FAILURE_ID {
        cx.report.bump("syntheticRowsDropped");
        return Ok(());
    }
    let created_at = norm_ts(cx, &m.created_at);
    let mut attachments = Vec::with_capacity(m.attachments.len());
    for a in m.attachments {
        attachments.push(conv_att(cx, a));
    }
    let msg = Message {
        seq: 0,
        id: m.id,
        crew_id: crew_id.to_string(),
        sender_kind: m.sender_kind,
        sender_session_id: m.sender_session_id,
        sender_name: m.sender_name,
        text: m.text,
        category: m.category,
        in_reply_to: m.in_reply_to,
        mentions: m
            .mentions
            .into_iter()
            .map(|x| Mention { kind: x.kind, target_id: x.target_id })
            .collect(),
        attachments,
        created_at,
    };
    match crewd_store::messages::insert(tx, &msg, Some(&Value::Object(m.extra)))? {
        Some(_) => cx.report.bump("messages"),
        None => cx.report.bump("duplicateMessages"),
    }
    Ok(())
}
```

`crates/legacy/src/todos.rs`：

```rust
use crate::Ctx;
use crate::model::LTodo;
use crate::rows::{conv_att, norm_ts, read_rows};
use crewd_proto::{Ledger, Todo, TodoResponse};
use crewd_store::StoreError;
use rusqlite::Connection;
use serde_json::Value;

pub fn import(tx: &Connection, cx: &mut Ctx, crew_id: &str, ledger: Ledger) -> crewd_store::Result<()> {
    let (suffix, count_key) = match ledger {
        Ledger::Agent => ("todos", "agentTodos"),
        Ledger::Human => ("human-todos", "humanTodos"),
    };
    let path = cx.src.join("whiteboards").join(format!("{crew_id}.{suffix}.json"));
    let rows: Vec<LTodo> = read_rows(cx, &path);
    for t in rows {
        let mut responses = Vec::with_capacity(t.responses.len());
        for r in t.responses {
            let mut atts = Vec::with_capacity(r.attachments.len());
            for a in r.attachments {
                atts.push(conv_att(cx, a));
            }
            let created_at = norm_ts(cx, &r.created_at);
            responses.push(TodoResponse {
                id: r.id,
                session_id: r.session_id,
                sender_name: r.sender_name,
                text: r.text,
                status: r.status,
                attachments: atts,
                created_at,
            });
        }
        let mut attachments = Vec::with_capacity(t.attachments.len());
        for a in t.attachments {
            attachments.push(conv_att(cx, a));
        }
        let created_at = norm_ts(cx, &t.created_at);
        let updated_at = t.updated_at.as_deref().map(|x| norm_ts(cx, x));
        let deleted_at = t.deleted_at.as_deref().map(|x| norm_ts(cx, x));
        let n_resp = responses.len();
        let todo = Todo {
            id: t.id,
            crew_id: crew_id.to_string(),
            ledger,
            number: t.number,
            text: t.text,
            status: t.status,
            created_at,
            updated_at,
            deleted_at,
            attachments,
            responses,
        };
        match crewd_store::todos::insert(tx, &todo, Some(&Value::Object(t.extra))) {
            Ok(()) => {
                cx.report.bump(count_key);
                cx.report.add("todoResponses", n_resp);
            }
            Err(StoreError::Sql(e)) if crewd_store::is_unique_violation(&e) => {
                cx.report.skip(cx.src, &path, None, format!("编号 #{} 或 id 重复: {e}", todo.number));
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
```

`crates/legacy/src/archive.rs`：

```rust
//! 没被建模导入、也不是缓存的文件，原样存进 legacy_archive（键 = 相对源目录的路径）。
use crate::Ctx;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// 只在数据根第一层跳过的目录。
const SKIP_TOP_DIRS: &[&str] = &[
    "attachments",
    "crashes",
    "backups",
    "diagnostics",
    "cli-maintenance-logs",
    "welcome-crew",
    "crewd",
];
/// 运行时缓存/镜像/锁：导入后无意义。
const SKIP_FILES: &[&str] = &[
    "crew-sessions.json",
    "quota.json",
    "models.json",
    "expected-live-sessions.json",
    "automatic-wake-admission.json",
    "app.lastexit.json",
    "orchestrator.lock",
    ".DS_Store",
];
const SKIP_SUFFIXES: &[&str] = &[
    ".lock",
    ".cursor",
    ".turn",
    ".crewresp.json",
    ".crewcmd.json",
    ".captain-delegation-seen",
];

fn is_runtime_file(name: &str) -> bool {
    SKIP_FILES.contains(&name)
        || name.starts_with("daemon.")
        || SKIP_SUFFIXES.iter().any(|s| name.ends_with(s))
}

pub fn import_rest(tx: &Connection, cx: &mut Ctx) -> crewd_store::Result<()> {
    let root = cx.src.to_path_buf();
    walk(tx, cx, &root, true)
}

fn walk(tx: &Connection, cx: &mut Ctx, dir: &Path, top: bool) -> crewd_store::Result<()> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            cx.report.skip(cx.src, dir, None, format!("目录读不出: {e}"));
            return Ok(());
        }
    };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if p.is_dir() {
            if top && SKIP_TOP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk(tx, cx, &p, false)?;
            continue;
        }
        if is_runtime_file(&name) || cx.consumed.contains(&p) {
            continue;
        }
        match std::fs::read(&p) {
            Ok(bytes) => {
                let key = p.strip_prefix(cx.src).unwrap_or(&p).display().to_string();
                crewd_store::archive::put(tx, &key, &bytes)?;
                cx.report.bump("archivedFiles");
            }
            Err(e) => cx.report.skip(cx.src, &p, None, format!("读取失败: {e}")),
        }
    }
    Ok(())
}
```

`lib.rs` 加 `mod archive; mod todos; mod whiteboard;`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-legacy`
Expected: 8 passed。

- [ ] **Step 5: 检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
cd ..
git add crewd/crates/legacy/src
git commit -m "crewd: import legacy whiteboards, todos and archive the rest

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 8: 组装 `crewd_legacy::import`

**Files:**
- Modify: `crewd/crates/legacy/src/lib.rs`
- Create: `crewd/crates/legacy/tests/import_basic.rs`

**Interfaces:**
- Consumes: Task 6/7 全部
- Produces: `pub fn import(src: &Path, store: &mut crewd_store::Store, dest_data_dir: &Path) -> anyhow::Result<ImportReport>`：在单个事务里导入，事务提交后复制附件到 `dest_data_dir/attachments`，并写 meta `importedAt`、`importSource`；源目录不存在 → Err。

- [ ] **Step 1: 写失败的测试**

`crates/legacy/tests/import_basic.rs`：

```rust
use crewd_legacy::import;
use crewd_store::Store;
use std::path::Path;

fn fixture() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/basic")
}

fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = vec![];
    fn walk(root: &Path, d: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                out.push((p.strip_prefix(root).unwrap().display().to_string(), std::fs::read(&p).unwrap()));
            }
        }
    }
    walk(dir, dir, &mut out);
    out.sort();
    out
}

#[test]
fn full_import_from_fixture() {
    let before = snapshot(&fixture());
    let dest = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dest.path().join("crewd.db")).unwrap();
    let r = import(&fixture(), &mut store, dest.path()).unwrap();

    assert_eq!(r.counts["crews"], 3);
    assert_eq!(r.counts["sessions"], 3);
    assert_eq!(r.counts["messages"], 3);
    assert_eq!(r.counts["agentTodos"], 2);
    assert_eq!(r.counts["humanTodos"], 1);
    assert_eq!(r.counts["attachmentFiles"], 1);
    assert!(dest.path().join("attachments/local-aaaa/note.txt").exists());
    assert!(crewd_store::meta_get(store.conn(), "importedAt").unwrap().is_some());
    assert_eq!(snapshot(&fixture()), before, "源目录必须一个字节都不变");
}

#[test]
fn missing_source_is_an_error_and_writes_nothing() {
    let dest = tempfile::tempdir().unwrap();
    let mut store = Store::open_in_memory().unwrap();
    assert!(import(&dest.path().join("nope"), &mut store, dest.path()).is_err());
    assert!(store.is_empty().unwrap());
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-legacy --test import_basic`
Expected: 编译失败（`import` 未定义）。

- [ ] **Step 3: 实现**

在 `crates/legacy/src/lib.rs` 的 `Ctx` 定义之后加：

```rust
/// 只读导入整个旧数据目录到 `store`（应为空库）。全部写入在一个事务里，失败则什么都不留。
/// 附件在事务提交后复制到 `dest_data_dir/attachments`。
pub fn import(
    src: &Path,
    store: &mut crewd_store::Store,
    dest_data_dir: &Path,
) -> anyhow::Result<ImportReport> {
    anyhow::ensure!(src.is_dir(), "旧数据目录不存在: {}", src.display());
    let mut report = ImportReport::default();
    let mut consumed = BTreeSet::new();
    let rewrite = paths::PathRewrite {
        from: src.join("attachments"),
        to: dest_data_dir.join("attachments"),
    };
    store.with_tx(|tx| {
        let mut cx = Ctx { src, report: &mut report, consumed: &mut consumed, rewrite: &rewrite };
        let crews = crews::import(tx, &mut cx)?;
        sessions::import(tx, &mut cx, &crews)?;
        for c in &crews {
            whiteboard::import_board(tx, &mut cx, &c.id)?;
            todos::import(tx, &mut cx, &c.id, crewd_proto::Ledger::Agent)?;
            todos::import(tx, &mut cx, &c.id, crewd_proto::Ledger::Human)?;
        }
        whiteboard::import_outbox(tx, &mut cx)?;
        archive::import_rest(tx, &mut cx)?;
        crewd_store::meta_set(tx, "importedAt", &crewd_store::now_ts())?;
        crewd_store::meta_set(tx, "importSource", &src.display().to_string())?;
        Ok(())
    })?;
    let copied = paths::copy_dir(&src.join("attachments"), &dest_data_dir.join("attachments"))?;
    report.add("attachmentFiles", copied);
    Ok(report)
}
```

若 Task 4 临时加过 `#[allow(dead_code)]`，此处删掉。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-legacy`
Expected: 单元 8 + 集成 2 全过。

- [ ] **Step 5: 检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
cd ..
git add crewd/crates/legacy
git commit -m "crewd: transactional legacy import entry point

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 9: server —— socket、分发、事件订阅、单实例锁

**Files:**
- Create: `crewd/crates/server/Cargo.toml`, `src/lib.rs`, `src/dispatch.rs`, `src/conn.rs`, `src/hub.rs`
- Create: `crewd/crates/server/tests/socket.rs`

**Interfaces:**
- Consumes: `crewd_proto::*`、`crewd_store::{Store, crews, messages, todos, now_ts}`
- Produces（crate `crewd_server`）:
  - `pub struct AppState { pub store: tokio::sync::Mutex<Store>, pub hub: Hub }`，`AppState::new(Store) -> AppState`
  - `pub struct Hub`：`publish(&self, Event)`、`subscribe(&self) -> broadcast::Receiver<Event>`
  - `pub async fn dispatch::handle(&AppState, Request) -> Response`
  - `pub async fn serve(data_dir: PathBuf, shutdown: impl Future<Output = ()>) -> anyhow::Result<()>`：建目录、拿 `crewd.lock` 独占锁（拿不到 → Err，消息含「另一个 crewd」）、删旧 socket、开 `crewd.db`、监听 `crewd.sock`，直到 `shutdown` 完成；退出时删 socket。

- [ ] **Step 1: 建 crate、加依赖**

```bash
cd crewd
cargo new --lib crates/server --name crewd-server --vcs none
# 改 Cargo.toml 为 workspace 继承
cargo add -p crewd-server tokio --features rt-multi-thread,macros,net,io-util,sync,time,signal
cargo add -p crewd-server serde_json anyhow uuid --features uuid/v4
cargo add -p crewd-server crewd-proto --path crates/proto
cargo add -p crewd-server crewd-store --path crates/store
cargo add -p crewd-server --dev tempfile
```

- [ ] **Step 2: 写失败的测试**

`crates/server/tests/socket.rs`：

```rust
use crewd_proto::{Crew, ServerFrame};
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::oneshot;

async fn start(dir: &Path) -> oneshot::Sender<()> {
    // 先建一个机组，post 需要它存在
    {
        let s = crewd_store::Store::open(&dir.join("crewd.db")).unwrap();
        crewd_store::crews::upsert(
            s.conn(),
            &Crew {
                id: "a".into(),
                title: "A".into(),
                number: Some(1),
                builtin: false,
                parent_ids: vec![],
                working_directory: None,
                captain_agent_kind: None,
                hidden_at: None,
                created_at: "2026-10-01T00:00:00.000Z".into(),
                updated_at: "2026-10-01T00:00:00.000Z".into(),
            },
            None,
        )
        .unwrap();
    }
    let (tx, rx) = oneshot::channel::<()>();
    let d = dir.to_path_buf();
    tokio::spawn(async move {
        crewd_server::serve(d, async {
            let _ = rx.await;
        })
        .await
        .unwrap();
    });
    for _ in 0..100 {
        if UnixStream::connect(dir.join("crewd.sock")).await.is_ok() {
            return tx;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("crewd 没起来");
}

struct Client {
    rd: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    wr: tokio::net::unix::OwnedWriteHalf,
}

impl Client {
    async fn connect(dir: &Path) -> Self {
        let s = UnixStream::connect(dir.join("crewd.sock")).await.unwrap();
        let (r, w) = s.into_split();
        Self { rd: BufReader::new(r).lines(), wr: w }
    }
    async fn send_raw(&mut self, line: &str) {
        self.wr.write_all(format!("{line}\n").as_bytes()).await.unwrap();
    }
    async fn next(&mut self) -> ServerFrame {
        let line = tokio::time::timeout(Duration::from_secs(5), self.rd.next_line())
            .await
            .expect("等响应超时")
            .unwrap()
            .expect("连接被关");
        serde_json::from_str(&line).unwrap()
    }
    async fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send_raw(&json!({"id":id,"method":method,"params":params}).to_string()).await;
        match self.next().await {
            ServerFrame::Response(r) => {
                assert_eq!(r.id, id);
                if let Some(e) = r.error {
                    panic!("{method} 出错: {}", e.message);
                }
                r.result.unwrap()
            }
            ServerFrame::Event(e) => panic!("意外的事件 {e:?}"),
        }
    }
}

#[tokio::test]
async fn hello_post_list_and_todo_list() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut c = Client::connect(dir.path()).await;
    let h = c.call(1, "system.hello", Value::Null).await;
    assert_eq!(h["apiVersion"], 1);
    assert_eq!(h["server"], "crewd");

    let crews = c.call(2, "crew.list", Value::Null).await;
    assert_eq!(crews[0]["id"], "a");

    let text = "发布 0.1.44 🚀，中文没问题";
    let m = c.call(3, "whiteboard.post", json!({"crewId":"a","text":text,"senderKind":"user"})).await;
    assert!(m["seq"].as_i64().unwrap() > 0);
    let list = c.call(4, "whiteboard.list", json!({"crewId":"a"})).await;
    assert_eq!(list[0]["text"], text);
    let none = c.call(5, "whiteboard.list", json!({"crewId":"a","afterSeq":m["seq"]})).await;
    assert_eq!(none, json!([]));

    let todos = c.call(6, "todo.list", json!({"crewId":"a","ledger":"agent"})).await;
    assert_eq!(todos, json!([]));
}

#[tokio::test]
async fn bad_requests_get_errors_and_connection_survives() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut c = Client::connect(dir.path()).await;

    c.send_raw("this is not json").await;
    match c.next().await {
        ServerFrame::Response(r) => {
            assert_eq!(r.id, 0);
            assert_eq!(r.error.unwrap().code, -32700);
        }
        other => panic!("{other:?}"),
    }
    c.send_raw(r#"{"id":2,"method":"no.such"}"#).await;
    match c.next().await {
        ServerFrame::Response(r) => assert_eq!(r.error.unwrap().code, -32601),
        other => panic!("{other:?}"),
    }
    c.send_raw(r#"{"id":3,"method":"whiteboard.post","params":{"crewId":"zz","text":"x","senderKind":"user"}}"#).await;
    match c.next().await {
        ServerFrame::Response(r) => assert_eq!(r.error.unwrap().code, -32602),
        other => panic!("{other:?}"),
    }
    c.send_raw(r#"{"id":4,"method":"whiteboard.post","params":{"crewId":"a","text":"  ","senderKind":"user"}}"#).await;
    match c.next().await {
        ServerFrame::Response(r) => assert_eq!(r.error.unwrap().code, -32602),
        other => panic!("{other:?}"),
    }
    let h = c.call(5, "system.hello", Value::Null).await;
    assert_eq!(h["apiVersion"], 1);
}

#[tokio::test]
async fn subscribers_receive_whiteboard_events() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut sub = Client::connect(dir.path()).await;
    let ok = sub.call(1, "events.subscribe", json!({"topics":["whiteboard"]})).await;
    assert_eq!(ok["topics"], json!(["whiteboard"]));

    let mut poster = Client::connect(dir.path()).await;
    poster.call(1, "whiteboard.post", json!({"crewId":"a","text":"通知我","senderKind":"user"})).await;

    match sub.next().await {
        ServerFrame::Event(e) => {
            assert_eq!(e.event, "whiteboard.appended");
            assert_eq!(e.data["text"], "通知我");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn second_instance_on_same_dir_is_refused_and_stale_socket_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("crewd.sock"), b"stale").unwrap();
    let _stop = start(dir.path()).await;
    let err = crewd_server::serve(dir.path().to_path_buf(), async {}).await.unwrap_err();
    assert!(err.to_string().contains("另一个 crewd"), "{err}");
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p crewd-server`
Expected: 编译失败（`serve` 未定义）。

- [ ] **Step 4: 实现**

`crates/server/src/hub.rs`：

```rust
use crewd_proto::Event;
use tokio::sync::broadcast;

/// 进程内事件总线。订阅者处理太慢会收到 Lagged，由连接层转成 `events.lagged`。
pub struct Hub {
    tx: broadcast::Sender<Event>,
}

impl Hub {
    pub fn new() -> Self {
        Self { tx: broadcast::channel(1024).0 }
    }
    pub fn publish(&self, e: Event) {
        // 没有订阅者时 send 会报错，这正常，忽略。
        let _ = self.tx.send(e);
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}
```

`crates/server/src/dispatch.rs`：

```rust
use crate::AppState;
use crewd_proto::{
    API_VERSION, Event, Hello, Message, Request, Response, RpcError, TodoListParams,
    WhiteboardListParams, WhiteboardPostParams, events, methods, parse_params,
};
use serde::Serialize;
use serde_json::Value;

const SENDER_KINDS: &[&str] = &["user", "session", "captain", "pendingcrew"];

pub async fn handle(state: &AppState, req: Request) -> Response {
    let id = req.id;
    match route(state, req).await {
        Ok(v) => Response::ok(id, v),
        Err(e) => Response::err(id, e),
    }
}

fn internal(e: impl std::fmt::Display) -> RpcError {
    RpcError::new(RpcError::INTERNAL, e.to_string())
}

fn invalid(msg: impl Into<String>) -> RpcError {
    RpcError::new(RpcError::INVALID_PARAMS, msg)
}

fn to_value<T: Serialize>(v: T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(internal)
}

async fn route(state: &AppState, req: Request) -> Result<Value, RpcError> {
    match req.method.as_str() {
        methods::HELLO => to_value(Hello {
            server: "crewd".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
        }),
        methods::CREW_LIST => {
            let s = state.store.lock().await;
            to_value(crewd_store::crews::list(s.conn()).map_err(internal)?)
        }
        methods::WHITEBOARD_LIST => {
            let p: WhiteboardListParams = parse_params(req.params)?;
            let limit = p.limit.unwrap_or(200).clamp(1, 1000);
            let s = state.store.lock().await;
            to_value(
                crewd_store::messages::list(s.conn(), &p.crew_id, p.after_seq.unwrap_or(0), limit)
                    .map_err(internal)?,
            )
        }
        methods::WHITEBOARD_POST => {
            let p: WhiteboardPostParams = parse_params(req.params)?;
            if p.text.trim().is_empty() {
                return Err(invalid("text 不能为空"));
            }
            if !SENDER_KINDS.contains(&p.sender_kind.as_str()) {
                return Err(invalid(format!("未知 senderKind: {}", p.sender_kind)));
            }
            let mut msg = Message {
                seq: 0,
                id: uuid::Uuid::new_v4().to_string(),
                crew_id: p.crew_id,
                sender_kind: p.sender_kind,
                sender_session_id: p.sender_session_id,
                sender_name: p.sender_name,
                text: p.text,
                category: None,
                in_reply_to: p.in_reply_to,
                mentions: p.mentions,
                attachments: vec![],
                created_at: crewd_store::now_ts(),
            };
            {
                let s = state.store.lock().await;
                if !crewd_store::crews::exists(s.conn(), &msg.crew_id).map_err(internal)? {
                    return Err(invalid(format!("机组不存在: {}", msg.crew_id)));
                }
                msg.seq = crewd_store::messages::insert(s.conn(), &msg, None)
                    .map_err(internal)?
                    .ok_or_else(|| internal("新消息 id 冲突"))?;
            }
            let v = to_value(&msg)?;
            state.hub.publish(Event { event: events::WHITEBOARD_APPENDED.into(), data: v.clone() });
            Ok(v)
        }
        methods::TODO_LIST => {
            let p: TodoListParams = parse_params(req.params)?;
            let s = state.store.lock().await;
            to_value(crewd_store::todos::list(s.conn(), &p.crew_id, p.ledger).map_err(internal)?)
        }
        other => Err(RpcError::new(RpcError::METHOD_NOT_FOUND, format!("未知方法: {other}"))),
    }
}
```

`crates/server/src/conn.rs`：

```rust
use crate::AppState;
use crewd_proto::{
    Event, Request, Response, RpcError, ServerFrame, SubscribeParams, events, methods,
    parse_params,
};
use serde_json::json;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

/// 一条连接：读一行请求 → 回一行响应；订阅的事件由独立任务推到同一个写通道。
pub async fn run(state: Arc<AppState>, stream: UnixStream) -> anyhow::Result<()> {
    let (rd, mut wr) = stream.into_split();
    let (tx, mut rx) = mpsc::channel::<ServerFrame>(256);
    let writer = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            let mut line = match serde_json::to_vec(&frame) {
                Ok(l) => l,
                Err(_) => continue,
            };
            line.push(b'\n');
            if wr.write_all(&line).await.is_err() {
                break;
            }
        }
    });

    let mut subs: Vec<JoinHandle<()>> = Vec::new();
    let mut lines = BufReader::new(rd).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let frame = match serde_json::from_str::<Request>(&line) {
            Err(e) => ServerFrame::Response(Response::err(
                0,
                RpcError::new(RpcError::PARSE, format!("无法解析请求: {e}")),
            )),
            Ok(req) if req.method == methods::SUBSCRIBE => {
                let id = req.id;
                match parse_params::<SubscribeParams>(req.params) {
                    Ok(p) => {
                        subs.push(spawn_forwarder(state.hub.subscribe(), p.topics.clone(), tx.clone()));
                        ServerFrame::Response(Response::ok(id, json!({ "topics": p.topics })))
                    }
                    Err(e) => ServerFrame::Response(Response::err(id, e)),
                }
            }
            Ok(req) => ServerFrame::Response(crate::dispatch::handle(&state, req).await),
        };
        if tx.send(frame).await.is_err() {
            break;
        }
    }
    for s in subs {
        s.abort();
    }
    drop(tx);
    let _ = writer.await;
    Ok(())
}

fn topic_of(event: &str) -> &str {
    event.split('.').next().unwrap_or(event)
}

fn spawn_forwarder(
    mut rx: broadcast::Receiver<Event>,
    topics: Vec<String>,
    tx: mpsc::Sender<ServerFrame>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let frame = match rx.recv().await {
                Ok(e) if topics.iter().any(|t| t == topic_of(&e.event)) => ServerFrame::Event(e),
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(n)) => ServerFrame::Event(Event {
                    event: events::LAGGED.into(),
                    data: json!({ "missed": n }),
                }),
                Err(broadcast::error::RecvError::Closed) => break,
            };
            if tx.send(frame).await.is_err() {
                break;
            }
        }
    })
}
```

`crates/server/src/lib.rs`：

```rust
//! crewd 服务端：Unix socket 上的 NDJSON RPC。
mod conn;
pub mod dispatch;
pub mod hub;

pub use hub::Hub;
use crewd_store::Store;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::UnixListener;

pub struct AppState {
    pub store: tokio::sync::Mutex<Store>,
    pub hub: Hub,
}

impl AppState {
    pub fn new(store: Store) -> Self {
        Self { store: tokio::sync::Mutex::new(store), hub: Hub::new() }
    }
}

pub async fn serve(data_dir: PathBuf, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
    std::fs::create_dir_all(&data_dir)?;
    let lock = std::fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(data_dir.join("crewd.lock"))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            anyhow::bail!("另一个 crewd 已在使用数据目录 {}", data_dir.display())
        }
        Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
    }
    // 拿到锁 = 没有别的实例在跑，残留的 socket 文件一定是上次崩溃留下的。
    let sock = data_dir.join("crewd.sock");
    let _ = std::fs::remove_file(&sock);

    let store = Store::open(&data_dir.join("crewd.db"))?;
    let state = Arc::new(AppState::new(store));
    let listener = UnixListener::bind(&sock)?;

    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let st = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = conn::run(st, stream).await {
                        eprintln!("crewd: 连接异常结束: {e}");
                    }
                });
            }
        }
    }
    let _ = std::fs::remove_file(&sock);
    drop(lock);
    Ok(())
}
```

`dispatch.rs` 用到 `serde::Serialize`：`cargo add -p crewd-server serde`。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd-server`
Expected: 4 passed。

- [ ] **Step 6: 检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
cd ..
git add crewd/Cargo.lock crewd/crates/server
git commit -m "crewd: NDJSON RPC server with subscriptions and single-instance lock

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 10: `crewd` 命令行 + 契约文件 + 端到端

**Files:**
- Create: `crewd/crates/crewd/Cargo.toml`, `src/main.rs`
- Create: `crewd/crates/crewd/tests/e2e.rs`
- Create: `crewd/schema/crewd-api.schema.json`（生成）
- Create: `crewd/README.md`

**Interfaces:**
- Consumes: `crewd_server::serve`、`crewd_legacy::{import, paths::default_legacy_root}`、`crewd_store::Store`、`crewd_proto::{api_schema, Request, ServerFrame}`
- Produces: 二进制 `crewd`，子命令：
  - `crewd serve` —— 前台跑，Ctrl-C / SIGTERM 退出
  - `crewd import [--from DIR]` —— 目标库已存在则拒绝；成功打印报告 JSON
  - `crewd rpc METHOD [PARAMS_JSON]` —— 打印 result；出错打印到 stderr 并以 1 退出
  - `crewd schema` —— 打印 `api_schema()`（美化 JSON）
  - 全局 `--data-dir DIR`（或 `CREWD_DATA_DIR`）

- [ ] **Step 1: 建 crate、加依赖**

```bash
cd crewd
cargo new --bin crates/crewd --name crewd --vcs none
# 改 Cargo.toml 为 workspace 继承
cargo add -p crewd clap --features derive,env
cargo add -p crewd tokio --features rt-multi-thread,macros,signal
cargo add -p crewd serde_json anyhow dirs
cargo add -p crewd crewd-proto --path crates/proto
cargo add -p crewd crewd-store --path crates/store
cargo add -p crewd crewd-legacy --path crates/legacy
cargo add -p crewd crewd-server --path crates/server
cargo add -p crewd --dev tempfile
```

- [ ] **Step 2: 写失败的测试**

`crates/crewd/tests/e2e.rs`：

```rust
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::time::{Duration, Instant};

fn crewd(data: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_crewd"));
    c.env("CREWD_DATA_DIR", data).env_remove("PENDINGCREW_DATA_DIR");
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
        if crewd(data).args(["rpc", "system.hello"]).output().unwrap().status.success() {
            return d;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("crewd serve 没起来");
}

fn rpc(data: &Path, method: &str, params: &str) -> serde_json::Value {
    let out: Output = crewd(data).args(["rpc", method, params]).output().unwrap();
    assert!(out.status.success(), "{method}: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn import_then_serve_then_rpc() {
    let data = tempfile::tempdir().unwrap();
    let out = crewd(data.path()).args(["import", "--from"]).arg(fixture()).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["counts"]["messages"], 3);

    let again = crewd(data.path()).args(["import", "--from"]).arg(fixture()).output().unwrap();
    assert!(!again.status.success(), "已有库时必须拒绝导入");

    let _d = start(data.path());
    let crews = rpc(data.path(), "crew.list", "null");
    assert_eq!(crews.as_array().unwrap().len(), 3);
    let before = rpc(data.path(), "whiteboard.list", r#"{"crewId":"local-aaaa"}"#);
    assert_eq!(before.as_array().unwrap().len(), 3);
    rpc(data.path(), "whiteboard.post", r#"{"crewId":"local-aaaa","text":"切换后第一条 ✅","senderKind":"user"}"#);
    let after = rpc(data.path(), "whiteboard.list", r#"{"crewId":"local-aaaa"}"#);
    assert_eq!(after[3]["text"], "切换后第一条 ✅");

    let second = crewd(data.path()).arg("serve").output().unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("另一个 crewd"));
}

#[test]
fn rpc_error_exits_nonzero() {
    let data = tempfile::tempdir().unwrap();
    let _d = start(data.path());
    let out = crewd(data.path()).args(["rpc", "no.such"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("未知方法"));
}

#[test]
fn committed_schema_matches_generated() {
    let generated = Command::new(env!("CARGO_BIN_EXE_crewd")).arg("schema").output().unwrap();
    assert!(generated.status.success());
    let committed = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema/crewd-api.schema.json"))
        .expect("缺 crewd/schema/crewd-api.schema.json，运行 `cargo run -p crewd -- schema > schema/crewd-api.schema.json`");
    let a: serde_json::Value = serde_json::from_slice(&generated.stdout).unwrap();
    let b: serde_json::Value = serde_json::from_slice(&committed).unwrap();
    assert_eq!(a, b, "API 契约变了：重新生成 schema/crewd-api.schema.json 并提交");
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p crewd`
Expected: 失败（默认 `main` 不认识子命令）。

- [ ] **Step 4: 实现**

`crates/crewd/src/main.rs`：

```rust
//! crewd：PendingCrew 的跨平台后端。
use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use crewd_proto::{Request, ServerFrame};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "crewd", version, about = "PendingCrew backend")]
struct Cli {
    /// 数据目录（库、socket、锁、附件都在这里）
    #[arg(long, global = true, env = "CREWD_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 前台运行服务
    Serve,
    /// 从旧版 JSON 数据只读导入到一个新库
    Import {
        /// 旧数据目录；缺省取 PENDINGCREW_DATA_DIR 或 ~/Library/Application Support/PendingCrew
        #[arg(long)]
        from: Option<PathBuf>,
    },
    /// 调用一个 RPC 方法并打印结果
    Rpc {
        method: String,
        /// JSON 参数，缺省 null
        params: Option<String>,
    },
    /// 打印 API 契约
    Schema,
}

fn data_dir(cli: &Cli) -> anyhow::Result<PathBuf> {
    match &cli.data_dir {
        Some(d) => Ok(d.clone()),
        None => Ok(dirs::data_dir().context("无法确定用户数据目录")?.join("crewd")),
    }
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("crewd: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let dir = data_dir(&cli)?;
    match &cli.cmd {
        Cmd::Serve => {
            crewd_server::serve(dir, shutdown_signal()).await?;
        }
        Cmd::Import { from } => {
            let src = from.clone().unwrap_or_else(crewd_legacy::paths::default_legacy_root);
            std::fs::create_dir_all(&dir)?;
            let db = dir.join("crewd.db");
            if db.exists() {
                bail!("{} 已存在。导入只写新库：先把它挪走再导入。", db.display());
            }
            let mut store = crewd_store::Store::open(&db)?;
            let report = crewd_legacy::import(&src, &mut store, &dir)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Cmd::Rpc { method, params } => {
            let params: serde_json::Value = match params {
                Some(p) => serde_json::from_str(p).context("PARAMS 不是合法 JSON")?,
                None => serde_json::Value::Null,
            };
            let result = call(&dir, method, params)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Cmd::Schema => {
            println!("{}", serde_json::to_string_pretty(&crewd_proto::api_schema())?);
        }
    }
    Ok(())
}

fn call(dir: &std::path::Path, method: &str, params: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let sock = dir.join("crewd.sock");
    let mut stream = UnixStream::connect(&sock)
        .with_context(|| format!("连不上 {}（crewd serve 在跑吗？）", sock.display()))?;
    let req = Request { id: 1, method: method.to_string(), params };
    let mut line = serde_json::to_vec(&req)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    let mut reader = BufReader::new(stream);
    loop {
        let mut buf = String::new();
        if reader.read_line(&mut buf)? == 0 {
            bail!("crewd 在回复前关闭了连接");
        }
        if let ServerFrame::Response(r) = serde_json::from_str::<ServerFrame>(&buf)? {
            if r.id != 1 {
                continue;
            }
            if let Some(e) = r.error {
                bail!("{} ({})", e.message, e.code);
            }
            return Ok(r.result.unwrap_or(serde_json::Value::Null));
        }
    }
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("无法监听 SIGTERM");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}
```

生成契约文件：

```bash
mkdir -p schema
cargo run -q -p crewd -- schema > schema/crewd-api.schema.json
```

`crewd/README.md`：

```markdown
# crewd

PendingCrew 的跨平台后端（Rust）。设计：`../docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`。

- 检查：`scripts/check.sh`
- 导入旧数据（只读源目录，只写新库）：`cargo run -p crewd -- import [--from DIR]`
- 运行：`cargo run -p crewd -- serve`
- 调用：`cargo run -p crewd -- rpc crew.list`
- API 契约：`schema/crewd-api.schema.json`（改了线协议类型后用 `cargo run -p crewd -- schema > schema/crewd-api.schema.json` 重新生成；测试会检查它是否过期）
- 数据目录：`--data-dir` > `CREWD_DATA_DIR` > `~/Library/Application Support/crewd`
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd`
Expected: 3 passed。

- [ ] **Step 6: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
cd ..
git add crewd/Cargo.lock crewd/crates/crewd crewd/schema crewd/README.md
git commit -m "crewd: CLI (serve/import/rpc/schema), committed API contract, e2e tests

Co-Authored-By: Claude <noreply@anthropic.com>"
```

- [ ] **Step 7: 真实数据只读演练（人工步骤，不提交任何产物）**

目的：确认导入器能吃下真实数据，只看计数和跳过原因，不看内容。写到临时目录，用完删掉。

```bash
cd crewd
T=$(mktemp -d)
cargo run -q -p crewd -- --data-dir "$T" import > "$T/report.json"; echo "exit=$?"
python3 -c "import json,sys; r=json.load(open('$T/report.json')); print(json.dumps(r['counts'],ensure_ascii=False,indent=1)); print('skipped:',len(r['skipped'])); [print(' ',s['file'],s['index'],s['reason'][:120]) for s in r['skipped'][:30]]; print('warnings:',r['warnings'])"
```

Expected: `exit=0`；crews/messages/todos 计数和旧版界面里看到的数量级一致；`skipped` 里每条原因都说得通。若出现 `Operation not permitted`（已知的数据目录周期性 EPERM，见记忆），记录下来并停止，不要绕；这不是导入器的 bug。

演练完：

```bash
rm -r "$T"
```

把计数（不含内容）和任何意外的跳过原因汇报给机长/人。若跳过原因暴露了 schema 理解错误，回到对应任务补测试修掉。
```
