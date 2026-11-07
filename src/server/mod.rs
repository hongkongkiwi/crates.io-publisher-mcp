use anyhow::{Result, anyhow};
use crates_io_api::AsyncClient;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParam, CallToolResult, Content, ListToolsResult, PaginatedRequestParam,
        ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::sync::Arc;
use std::time::Duration;
pub mod tools;

use crate::DEFAULT_BASE_URL;
use crate::USER_AGENT;
use crate::api::WriteClient;
use crate::auth::{Token, resolve_token};
use crate::mode::Mode;
use crate::ops::CargoOps;
use tools::{
    AUTH_STATUS, CHECK_CRATE_NAME, GetCrateDependenciesTool, GetCrateDocumentationTool,
    GetCrateInfoTool, GetCrateVersionsTool, PublishTools, SearchCratesTool, WriteTools,
    dependencies::CrateDependenciesInput, info::CrateNameInput, search::SearchCratesInput,
};

const RATE_LIMIT_MS: u64 = 100;

/// How the server was started. Tokens are resolved once, at startup.
#[derive(Clone)]
pub struct ServerConfig {
    pub mode: Mode,
    pub token: Option<Token>,
    pub base_url: String,
}

impl ServerConfig {
    pub fn new(mode: Mode) -> Result<Self> {
        let token = if mode.allows_write() {
            let token = resolve_token()?.ok_or_else(|| {
                anyhow!(
                    "`--mode {mode}` needs a crates.io API token. Set CRATES_IO_TOKEN or \
                     CARGO_REGISTRY_TOKEN, or run `cargo login`. Create a scoped token at \
                     https://crates.io/settings/tokens with the scopes you need: \
                     publish-new, publish-update, yank, change-owners, trusted-publishing."
                )
            })?;
            Some(token)
        } else {
            None
        };

        let base_url =
            std::env::var("CRATES_MCP_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());

        Ok(Self {
            mode,
            token,
            base_url,
        })
    }
}

/// MCP Server for interacting with crates.io
#[derive(Clone)]
pub struct CratesIoServer {
    client: Arc<AsyncClient>,
    mode: Mode,
    base_url: String,
    token: Option<Token>,
    search_crates: Arc<SearchCratesTool>,
    get_crate_info: Arc<GetCrateInfoTool>,
    get_crate_versions: Arc<GetCrateVersionsTool>,
    get_crate_dependencies: Arc<GetCrateDependenciesTool>,
    get_crate_documentation: Arc<GetCrateDocumentationTool>,
    write: Option<Arc<WriteTools>>,
    publish: Option<Arc<PublishTools>>,
}

impl CratesIoServer {
    /// Create a new CratesIoServer instance
    pub fn new(config: ServerConfig) -> Result<Self> {
        let client = Arc::new(AsyncClient::new(
            USER_AGENT,
            Duration::from_millis(RATE_LIMIT_MS),
        )?);

        let write = if config.mode.allows_write() {
            let token = config.token.clone().ok_or_else(|| {
                anyhow!("write mode requires a token, but none was resolved at startup")
            })?;
            let write_client = WriteClient::new(token.expose(), &config.base_url)?;
            Some(Arc::new(WriteTools::new(Arc::new(write_client))))
        } else {
            None
        };

        let publish = if config.mode.allows_publish() {
            let token = config.token.clone().ok_or_else(|| {
                anyhow!("publish mode requires a token, but none was resolved at startup")
            })?;
            Some(Arc::new(PublishTools::new(
                Arc::new(CargoOps::default()),
                token,
            )))
        } else {
            None
        };

        Ok(Self {
            client,
            mode: config.mode,
            base_url: config.base_url,
            token: config.token,
            search_crates: Arc::new(SearchCratesTool::default()),
            get_crate_info: Arc::new(GetCrateInfoTool::default()),
            get_crate_versions: Arc::new(GetCrateVersionsTool::default()),
            get_crate_dependencies: Arc::new(GetCrateDependenciesTool::default()),
            get_crate_documentation: Arc::new(GetCrateDocumentationTool::default()),
            write,
            publish,
        })
    }

    fn tools(&self) -> Vec<Tool> {
        let mut tools = vec![
            self.search_crates.inner.clone(),
            self.get_crate_info.inner.clone(),
            self.get_crate_versions.inner.clone(),
            self.get_crate_dependencies.inner.clone(),
            self.get_crate_documentation.inner.clone(),
            Tool::new(
                CHECK_CRATE_NAME,
                "Check whether a crate name is valid and currently unused on crates.io. This is advisory: reserved names and recently deleted crates can still be rejected at publish time.",
                tools::schema_for::<CrateNameInput>(),
            ),
            Tool::new(
                AUTH_STATUS,
                "Report the server mode, whether a crates.io token was found, and where it came from. The token value is never returned. crates.io has no token introspection endpoint, so scopes can only be confirmed by creating the token with the right scopes.",
                tools::schema_for::<EmptyInput>(),
            ),
        ];

        if let Some(write) = &self.write {
            tools.extend(write.tools());
        }
        if let Some(publish) = &self.publish {
            tools.extend(publish.tools());
        }

        tools
    }

    async fn dispatch_read(&self, name: &str, args: Map<String, Value>) -> Result<CallToolResult> {
        match name {
            SearchCratesTool::NAME => {
                let input: SearchCratesInput = Self::deserialize_input(args)?;
                self.search_crates.execute(&self.client, input).await
            }
            GetCrateInfoTool::NAME => {
                let input: CrateNameInput = Self::deserialize_input(args)?;
                self.get_crate_info.execute(&self.client, input).await
            }
            GetCrateVersionsTool::NAME => {
                let input: CrateNameInput = Self::deserialize_input(args)?;
                self.get_crate_versions.execute(&self.client, input).await
            }
            GetCrateDependenciesTool::NAME => {
                let input: CrateDependenciesInput = Self::deserialize_input(args)?;
                self.get_crate_dependencies
                    .execute(&self.client, input)
                    .await
            }
            GetCrateDocumentationTool::NAME => {
                let input: CrateNameInput = Self::deserialize_input(args)?;
                self.get_crate_documentation
                    .execute(&self.client, input)
                    .await
            }
            CHECK_CRATE_NAME => {
                let input: CrateNameInput = Self::deserialize_input(args)?;
                self.check_crate_name(input).await
            }
            AUTH_STATUS => Ok(self.auth_status()),
            _ => Err(anyhow!("Unknown tool: {name}")),
        }
    }

    async fn check_crate_name(&self, input: CrateNameInput) -> Result<CallToolResult> {
        if let Err(error) = crate::ops::validate_crate_name(&input.name) {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "`{}` is not a valid crate name: {error}",
                input.name
            ))]));
        }

        match self.client.get_crate(&input.name).await {
            Ok(response) => Ok(CallToolResult::success(vec![Content::text(format!(
                "`{}` is already published (latest version {}). Choose another name.",
                input.name, response.crate_data.max_version
            ))])),
            Err(crates_io_api::Error::NotFound(_)) => {
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "`{}` is valid and not currently published. A recently deleted crate keeps its name for a cooldown period, and some names are reserved, so the registry can still reject it at publish time.",
                    input.name
                ))]))
            }
            Err(error) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error checking crate name: {error}"
            ))])),
        }
    }

    fn auth_status(&self) -> CallToolResult {
        let mut lines = vec![format!("mode: {}", self.mode)];

        match (&self.token, self.mode) {
            (Some(token), _) => {
                lines.push("token: present".to_string());
                lines.push(format!("source: {}", token.source().description()));
                if let Some(path) = token.location() {
                    lines.push(format!("credentials file: {}", path.display()));
                }
                lines.push(
                    "scopes: cannot be queried. crates.io has no token introspection endpoint, so a missing scope appears as HTTP 403 on the first write. Confirm scopes when creating the token at https://crates.io/settings/tokens."
                        .to_string(),
                );
            }
            (None, Mode::ReadOnly) => lines.push("token: not read (read-only mode)".to_string()),
            (None, _) => lines.push("token: missing".to_string()),
        }

        lines.push(format!("api: {}", self.base_url));
        CallToolResult::success(vec![Content::text(lines.join("\n"))])
    }

    async fn call_tool_inner(
        &self,
        name: &str,
        args: Map<String, Value>,
    ) -> Result<CallToolResult> {
        let required = tools::required_mode(name).ok_or_else(|| anyhow!("Unknown tool: {name}"))?;

        if self.mode < required {
            return Err(anyhow!(
                "Tool `{name}` requires `--mode {required}`, but this server runs in `{}` mode. Restart it with `--mode {required}` to enable this tool.",
                self.mode
            ));
        }

        match required {
            Mode::ReadOnly => self.dispatch_read(name, args).await,
            Mode::Write => self
                .write
                .as_ref()
                .expect("write tools exist when mode allows write")
                .execute(name, args)
                .await?
                .ok_or_else(|| anyhow!("Unknown tool: {name}")),
            Mode::Publish => self
                .publish
                .as_ref()
                .expect("publish tools exist when mode allows publish")
                .execute(name, args)
                .await?
                .ok_or_else(|| anyhow!("Unknown tool: {name}")),
        }
    }

    fn deserialize_input<T: DeserializeOwned>(args: Map<String, Value>) -> Result<T> {
        serde_json::from_value(Value::Object(args))
            .map_err(|error| anyhow!("invalid parameters: {error}"))
    }
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
struct EmptyInput {}

// Note: list_tools and call_tool use `impl Future` syntax instead of `async fn`
// because the ServerHandler trait requires this signature pattern.
#[allow(clippy::manual_async_fn)]
impl ServerHandler for CratesIoServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "MCP server for crates.io, the Rust package registry. Tool availability depends \
                 on the mode this process was started with: read-only, write, or publish. Call \
                 crates_auth_status to see the current mode and token source. Write and publish \
                 tools change state on crates.io and publishing is irreversible after 72 hours."
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

            self.call_tool_inner(name, args)
                .await
                .map_err(|error| McpError::internal_error(format!("{error:#}"), None))
        }
    }
}
