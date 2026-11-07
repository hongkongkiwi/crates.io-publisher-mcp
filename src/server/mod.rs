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
use crate::api::{SessionClient, WriteClient};
use crate::auth::{Cookie, Token, resolve_cookie, resolve_token};
use crate::credentials;
use crate::mode::Mode;
use crate::ops::CargoOps;
use tools::{
    AUTH_STATUS, AdminTools, CHECK_CRATE_NAME, GetCrateDependenciesTool, GetCrateDocumentationTool,
    GetCrateInfoTool, GetCrateVersionsTool, PublishTools, SearchCratesTool, WriteTools,
    dependencies::CrateDependenciesInput, info::CrateNameInput, search::SearchCratesInput,
};

const RATE_LIMIT_MS: u64 = 100;

/// How the server was started. Credentials are resolved once, at startup.
#[derive(Clone)]
pub struct ServerConfig {
    pub mode: Mode,
    pub token: Option<Token>,
    pub cookie: Option<Cookie>,
    pub base_url: String,
}

impl ServerConfig {
    pub fn new(mode: Mode) -> Result<Self> {
        let token = if mode.allows_write() {
            match resolve_token() {
                Ok(token) => token,
                // The token is optional in admin mode, so an unrelated broken
                // credentials file must not block a cookie-only deployment.
                Err(error) if mode == Mode::Admin => {
                    tracing::warn!(
                        "ignoring token resolution error in admin mode; continuing with the session cookie: {error:#}"
                    );
                    None
                }
                Err(error) => return Err(error),
            }
        } else {
            None
        };

        if mode.allows_write() && mode != Mode::Admin && token.is_none() {
            return Err(anyhow!(
                "`--mode {mode}` needs a crates.io API token. Run `cratesdotio-mcp login`, set \
                 CRATES_IO_TOKEN or CARGO_REGISTRY_TOKEN, or run `cargo login`. Create a scoped \
                 token at https://crates.io/settings/tokens with the scopes you need: \
                 publish-new, publish-update, yank, change-owners, trusted-publishing."
            ));
        }

        let cookie = if mode.allows_admin() {
            Some(resolve_cookie()?.ok_or_else(|| {
                anyhow!(
                    "`--mode admin` needs a crates.io session cookie. Run `cratesdotio-mcp session` \
                     to store one, or set CRATES_IO_COOKIE. The cookie is full account authority."
                )
            })?)
        } else {
            None
        };

        if mode == Mode::Admin && token.is_none() {
            tracing::info!(
                "admin mode has a session cookie but no API token; write and publish tools are unavailable"
            );
        }

        let base_url =
            std::env::var("CRATES_MCP_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());

        Ok(Self {
            mode,
            token,
            cookie,
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
    cookie: Option<Cookie>,
    search_crates: Arc<SearchCratesTool>,
    get_crate_info: Arc<GetCrateInfoTool>,
    get_crate_versions: Arc<GetCrateVersionsTool>,
    get_crate_dependencies: Arc<GetCrateDependenciesTool>,
    get_crate_documentation: Arc<GetCrateDocumentationTool>,
    write: Option<Arc<WriteTools>>,
    publish: Option<Arc<PublishTools>>,
    admin: Option<Arc<AdminTools>>,
}

impl CratesIoServer {
    /// Create a new CratesIoServer instance
    pub fn new(config: ServerConfig) -> Result<Self> {
        let client = Arc::new(AsyncClient::new(
            USER_AGENT,
            Duration::from_millis(RATE_LIMIT_MS),
        )?);

        let write = match (config.mode.allows_write(), config.token.as_ref()) {
            (true, Some(token)) => {
                let write_client = WriteClient::new(token.expose(), &config.base_url)?;
                Some(Arc::new(WriteTools::new(Arc::new(write_client))))
            }
            _ => None,
        };

        let publish = match (config.mode.allows_publish(), config.token.clone()) {
            (true, Some(token)) => Some(Arc::new(PublishTools::new(
                Arc::new(CargoOps::default()),
                token,
            ))),
            _ => None,
        };

        let admin = match (config.mode.allows_admin(), config.cookie.as_ref()) {
            (true, Some(cookie)) => {
                let session_client = SessionClient::new(cookie.expose(), &config.base_url)?;
                Some(Arc::new(AdminTools::new(
                    Arc::new(session_client),
                    credentials::config_dir(),
                )))
            }
            _ => None,
        };

        Ok(Self {
            client,
            mode: config.mode,
            base_url: config.base_url,
            token: config.token,
            cookie: config.cookie,
            search_crates: Arc::new(SearchCratesTool::default()),
            get_crate_info: Arc::new(GetCrateInfoTool::default()),
            get_crate_versions: Arc::new(GetCrateVersionsTool::default()),
            get_crate_dependencies: Arc::new(GetCrateDependenciesTool::default()),
            get_crate_documentation: Arc::new(GetCrateDocumentationTool::default()),
            write,
            publish,
            admin,
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
                "Report the server mode, whether a crates.io token and admin session cookie were found, and where they came from. Values are never returned. crates.io cannot map a token value to its scopes, so scopes are only confirmed by the operation that needs them.",
                tools::schema_for::<EmptyInput>(),
            ),
        ];

        if let Some(write) = &self.write {
            tools.extend(write.tools());
        }
        if let Some(publish) = &self.publish {
            tools.extend(publish.tools());
        }
        if let Some(admin) = &self.admin {
            tools.extend(admin.tools());
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
                    "`{}` is valid and not currently published. A recently deleted crate keeps its name for 24 hours, and some names are reserved, so the registry can still reject it at publish time.",
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

        if self.mode.allows_admin() {
            match &self.cookie {
                Some(cookie) => {
                    lines.push("session cookie: present".to_string());
                    lines.push(format!("cookie source: {}", cookie.source().description()));
                    if let Some(path) = cookie.location() {
                        lines.push(format!("session file: {}", path.display()));
                    }
                    lines.push(
                        "session scope: full account authority (token creation, crate deletion, account settings)"
                            .to_string(),
                    );
                }
                None => lines.push("session cookie: missing".to_string()),
            }
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
                .ok_or_else(|| {
                    anyhow!(
                        "Tool `{name}` needs an API token. Run `cratesdotio-mcp login`, or set \
                         CRATES_IO_TOKEN."
                    )
                })?
                .execute(name, args)
                .await?
                .ok_or_else(|| anyhow!("Unknown tool: {name}")),
            Mode::Publish => self
                .publish
                .as_ref()
                .ok_or_else(|| {
                    anyhow!(
                        "Tool `{name}` needs an API token. Run `cratesdotio-mcp login`, or set \
                         CRATES_IO_TOKEN."
                    )
                })?
                .execute(name, args)
                .await?
                .ok_or_else(|| anyhow!("Unknown tool: {name}")),
            Mode::Admin => self
                .admin
                .as_ref()
                .ok_or_else(|| {
                    anyhow!(
                        "Tool `{name}` needs a session cookie. Run `cratesdotio-mcp session`, or \
                         set CRATES_IO_COOKIE."
                    )
                })?
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
                 on the mode this process was started with: read-only, write, publish, or admin. \
                 Call crates_auth_status to see the current mode, token source, and whether an \
                 admin session cookie is loaded. Write and publish tools change state on \
                 crates.io; publishing is irreversible after 72 hours, and crate deletion is \
                 permanent."
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
