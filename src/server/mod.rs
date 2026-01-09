use anyhow::Result;
use crates_io_api::AsyncClient;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParam, CallToolResult, ListToolsResult, PaginatedRequestParam,
        ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
};
use std::sync::Arc;
use std::time::Duration;

pub mod tools;

use tools::{
    GetCrateDependenciesTool, GetCrateDocumentationTool, GetCrateInfoTool, GetCrateVersionsTool,
    SearchCratesTool, dependencies::CrateDependenciesInput, info::CrateNameInput,
    search::SearchCratesInput,
};

const USER_AGENT: &str = "cratesdotio-mcp/0.1.0 (MCP Server for crates.io)";
const RATE_LIMIT_MS: u64 = 100;

/// MCP Server for interacting with crates.io
#[derive(Clone)]
pub struct CratesIoServer {
    client: Arc<AsyncClient>,
    search_crates: Arc<SearchCratesTool>,
    get_crate_info: Arc<GetCrateInfoTool>,
    get_crate_versions: Arc<GetCrateVersionsTool>,
    get_crate_dependencies: Arc<GetCrateDependenciesTool>,
    get_crate_documentation: Arc<GetCrateDocumentationTool>,
}

impl CratesIoServer {
    /// Create a new CratesIoServer instance
    pub fn new() -> Result<Self> {
        let client = Arc::new(AsyncClient::new(
            USER_AGENT,
            Duration::from_millis(RATE_LIMIT_MS),
        )?);

        Ok(Self {
            client,
            search_crates: Arc::new(SearchCratesTool::new()),
            get_crate_info: Arc::new(GetCrateInfoTool::new()),
            get_crate_versions: Arc::new(GetCrateVersionsTool::new()),
            get_crate_dependencies: Arc::new(GetCrateDependenciesTool::new()),
            get_crate_documentation: Arc::new(GetCrateDocumentationTool::new()),
        })
    }

    fn tools(&self) -> Vec<Tool> {
        vec![
            self.search_crates.inner.clone(),
            self.get_crate_info.inner.clone(),
            self.get_crate_versions.inner.clone(),
            self.get_crate_dependencies.inner.clone(),
            self.get_crate_documentation.inner.clone(),
        ]
    }
}

// Note: list_tools and call_tool use `impl Future` syntax instead of `async fn`
// because the ServerHandler trait requires this signature pattern.
#[allow(clippy::manual_async_fn)]
impl ServerHandler for CratesIoServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "MCP server for interacting with crates.io, the Rust package registry. \
                Use the available tools to search for crates, get crate information, \
                list versions, view dependencies, and find documentation."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        async {
            Ok(ListToolsResult {
                tools: self.tools(),
                next_cursor: None,
                meta: None,
            })
        }
    }

    fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        async move {
            let name = request.name.as_ref();
            let args = request.arguments.unwrap_or_default();

            let result = match name {
                "search_crates" => {
                    let input: SearchCratesInput =
                        serde_json::from_value(serde_json::Value::Object(args)).map_err(|e| {
                            McpError::invalid_params(format!("Invalid parameters: {}", e), None)
                        })?;
                    self.search_crates.execute(&self.client, input).await
                }
                "get_crate_info" => {
                    let input: CrateNameInput =
                        serde_json::from_value(serde_json::Value::Object(args)).map_err(|e| {
                            McpError::invalid_params(format!("Invalid parameters: {}", e), None)
                        })?;
                    self.get_crate_info.execute(&self.client, input).await
                }
                "get_crate_versions" => {
                    let input: CrateNameInput =
                        serde_json::from_value(serde_json::Value::Object(args)).map_err(|e| {
                            McpError::invalid_params(format!("Invalid parameters: {}", e), None)
                        })?;
                    self.get_crate_versions.execute(&self.client, input).await
                }
                "get_crate_dependencies" => {
                    let input: CrateDependenciesInput =
                        serde_json::from_value(serde_json::Value::Object(args)).map_err(|e| {
                            McpError::invalid_params(format!("Invalid parameters: {}", e), None)
                        })?;
                    self.get_crate_dependencies
                        .execute(&self.client, input)
                        .await
                }
                "get_crate_documentation" => {
                    let input: CrateNameInput =
                        serde_json::from_value(serde_json::Value::Object(args)).map_err(|e| {
                            McpError::invalid_params(format!("Invalid parameters: {}", e), None)
                        })?;
                    self.get_crate_documentation
                        .execute(&self.client, input)
                        .await
                }
                _ => {
                    return Err(McpError::invalid_params(
                        format!("Unknown tool: {}", name),
                        None,
                    ));
                }
            };

            match result {
                Ok(res) => Ok(res),
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            }
        }
    }
}
