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
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
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
            search_crates: Arc::new(SearchCratesTool::default()),
            get_crate_info: Arc::new(GetCrateInfoTool::default()),
            get_crate_versions: Arc::new(GetCrateVersionsTool::default()),
            get_crate_dependencies: Arc::new(GetCrateDependenciesTool::default()),
            get_crate_documentation: Arc::new(GetCrateDocumentationTool::default()),
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

    async fn on_search_crates(&self, args: Map<String, Value>) -> Result<CallToolResult> {
        let input: SearchCratesInput = Self::deserialize_input(args)?;
        self.search_crates.execute(&self.client, input).await
    }

    async fn on_get_crate_info(&self, args: Map<String, Value>) -> Result<CallToolResult> {
        let input: CrateNameInput = Self::deserialize_input(args)?;
        self.get_crate_info.execute(&self.client, input).await
    }

    async fn on_get_crate_versions(&self, args: Map<String, Value>) -> Result<CallToolResult> {
        let input: CrateNameInput = Self::deserialize_input(args)?;
        self.get_crate_versions.execute(&self.client, input).await
    }

    async fn on_get_crate_dependencies(&self, args: Map<String, Value>) -> Result<CallToolResult> {
        let input: CrateDependenciesInput = Self::deserialize_input(args)?;
        self.get_crate_dependencies
            .execute(&self.client, input)
            .await
    }

    async fn on_get_crate_documentation(&self, args: Map<String, Value>) -> Result<CallToolResult> {
        let input: CrateNameInput = Self::deserialize_input(args)?;
        self.get_crate_documentation
            .execute(&self.client, input)
            .await
    }

    fn deserialize_input<T: DeserializeOwned>(args: Map<String, Value>) -> Result<T, McpError> {
        serde_json::from_value(Value::Object(args))
            .map_err(|e| McpError::invalid_params(format!("Invalid parameters: {}", e), None))
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
                SearchCratesTool::NAME => self.on_search_crates(args).await,
                GetCrateInfoTool::NAME => self.on_get_crate_info(args).await,
                GetCrateVersionsTool::NAME => self.on_get_crate_versions(args).await,
                GetCrateDependenciesTool::NAME => self.on_get_crate_dependencies(args).await,
                GetCrateDocumentationTool::NAME => self.on_get_crate_documentation(args).await,
                _ => {
                    return Err(McpError::internal_error(
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
