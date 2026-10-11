//! `crewd mcp`：每个 agent session 的 stdio MCP server（spec §22.3）。哑管道：工具表与执行都在 serve。
//! 用 rmcp 3.5 手写 `ServerHandler`（不用 `#[tool]` 宏：工具表来自 crewd，按角色变）。
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crewd_proto::agent::{CallResult, TOOLS_CHANGED, ToolDef, ToolsChanged, methods};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData,
    Implementation, InitializeResult, ListToolsResult, PaginatedRequestParams, ServerCapabilities,
    ServerConfig, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler, ServiceExt};
use serde_json::{Value, json};

use crate::agent_client::{AgentClient, CallError};

#[derive(clap::Args, Clone)]
pub struct McpArgs {
    /// crewd session id
    #[arg(long)]
    session: String,
    /// `agent.attach` 用的 token 所在文件（0600，crewd 起会话时写）
    #[arg(long)]
    token_file: PathBuf,
}

/// 给 agent 看的失败说明（helper 的英文句子，spec §22.3 决定 1）：分清「没做」「被拒」「不知道做没做」。
fn call_failure_text(e: &CallError) -> String {
    match e {
        CallError::NotSent(m) => {
            format!("crewd is not reachable ({m}); this call was not sent and did nothing")
        }
        CallError::Rejected { code, message } => {
            format!("crewd rejected this call: {message} ({code})")
        }
        CallError::Lost(m) => format!(
            "the connection to crewd was lost after this call was sent ({m}); \
             it may or may not have been done, check before retrying"
        ),
        CallError::BadReply(m) => format!(
            "crewd answered but the answer could not be read ({m}); \
             the call was probably done, check before retrying"
        ),
    }
}

#[derive(Clone)]
struct Bridge {
    client: Arc<AgentClient>,
    instructions: String,
    version: String,
}

fn to_tool(d: ToolDef) -> Tool {
    let schema = match d.input_schema {
        Value::Object(m) => m,
        _ => Default::default(),
    };
    let tool = Tool::new(d.name, d.description, Arc::new(schema));
    match d.annotations {
        Some(h) => tool.with_annotations(
            ToolAnnotations::new()
                .read_only(h.read_only_hint)
                .destructive(h.destructive_hint)
                .idempotent(h.idempotent_hint)
                .open_world(h.open_world_hint),
        ),
        None => tool,
    }
}

impl ServerHandler for Bridge {
    fn get_info(&self) -> ServerConfig {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .build(),
        )
        .with_server_info(Implementation::new("crewd", &self.version))
        .with_instructions(&self.instructions)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let defs: Vec<ToolDef> = self
            .client
            .call(methods::TOOLS, Value::Null)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(ListToolsResult::with_all_items(
            defs.into_iter().map(to_tool).collect(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args = request.arguments.map(Value::Object).unwrap_or(Value::Null);
        let r = match self
            .client
            .call::<CallResult>(
                methods::CALL,
                json!({"tool": request.name.to_string(), "args": args}),
            )
            .await
        {
            Ok(r) => r,
            Err(e) => CallResult::err(call_failure_text(&e)),
        };
        let content = vec![ContentBlock::text(r.text)];
        let result = if r.is_error {
            CallToolResult::error(content)
        } else {
            CallToolResult::success(content)
        };
        Ok(result.into())
    }
}

pub async fn run(data_dir: &Path, args: McpArgs) -> anyhow::Result<()> {
    let (client, who) = AgentClient::connect(data_dir, &args.session, &args.token_file).await?;
    eprintln!(
        "crewd mcp: attached {} ({:?}) in {} surface {}",
        args.session, who.role, who.crew_id, who.surface_hash
    );
    // Keep the original instructions and public tool surface. Only recognized Skew
    // appends the daemon's localized warning; old/future/Unknown checks do not.
    let instructions = match who.skew_warning() {
        Some(warning) => format!("{}\n\n{warning}", who.instructions),
        None => who.instructions.clone(),
    };
    let client = Arc::new(client);
    let mut events = client.take_events().expect("events taken once");
    let service = Bridge {
        client,
        instructions,
        version: format!("{}+{}", env!("CARGO_PKG_VERSION"), who.surface_hash),
    }
    .serve(rmcp::transport::stdio())
    .await?;
    let peer = service.peer().clone();
    tokio::spawn(async move {
        while let Some(e) = events.recv().await {
            if e.event != TOOLS_CHANGED {
                continue;
            }
            // `data` 是 `ToolsChanged { role }`；看不懂也照样通知（工具表以 serve 为准，重取即可）。
            match serde_json::from_value::<ToolsChanged>(e.data) {
                Ok(tc) => eprintln!("crewd mcp: role is now {:?}, tool list changed", tc.role),
                Err(err) => eprintln!("crewd mcp: unexpected tools_changed payload: {err}"),
            }
            if let Err(err) = peer.notify_tool_list_changed().await {
                eprintln!("crewd mcp: list_changed not delivered: {err}");
            }
        }
    });
    service.waiting().await?;
    Ok(())
}
