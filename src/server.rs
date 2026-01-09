use anyhow::Result;
use crates_io_api::{AsyncClient, CratesQuery, Sort};
use rmcp::{
    Error as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParam, CallToolResult, Content, ListToolsResult, PaginatedRequestParam,
        ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

const USER_AGENT: &str = "cratesdotio-mcp/0.1.0 (MCP Server for crates.io)";
const RATE_LIMIT_MS: u64 = 100;

/// MCP Server for interacting with crates.io
#[derive(Clone)]
pub struct CratesIoServer {
    client: Arc<AsyncClient>,
}

// Tool input parameter structures
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchCratesInput {
    /// Search query string
    pub query: String,
    /// Maximum number of results (default: 10, max: 100)
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_limit() -> u32 {
    10
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CrateNameInput {
    /// Name of the crate
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CrateDependenciesInput {
    /// Name of the crate
    pub name: String,
    /// Version of the crate (e.g., "1.0.0")
    pub version: String,
}

fn schema_for<T: JsonSchema>() -> Arc<serde_json::Map<String, serde_json::Value>> {
    let schema = schemars::schema_for!(T);
    match serde_json::to_value(schema).unwrap() {
        serde_json::Value::Object(m) => Arc::new(m),
        _ => panic!("schema should be an object"),
    }
}

impl CratesIoServer {
    /// Create a new CratesIoServer instance
    pub fn new() -> Result<Self> {
        let client = AsyncClient::new(USER_AGENT, Duration::from_millis(RATE_LIMIT_MS))?;
        Ok(Self {
            client: Arc::new(client),
        })
    }

    fn tools() -> Vec<Tool> {
        vec![
            Tool {
                name: Cow::Borrowed("search_crates"),
                description: Cow::Borrowed(
                    "Search for crates on crates.io by name or keyword. Returns a list of matching crates with their descriptions and download counts.",
                ),
                input_schema: schema_for::<SearchCratesInput>(),
            },
            Tool {
                name: Cow::Borrowed("get_crate_info"),
                description: Cow::Borrowed(
                    "Get detailed information about a specific crate including description, homepage, repository, downloads, and latest version.",
                ),
                input_schema: schema_for::<CrateNameInput>(),
            },
            Tool {
                name: Cow::Borrowed("get_crate_versions"),
                description: Cow::Borrowed(
                    "Get a list of all published versions of a crate, including version numbers, publication dates, and download counts.",
                ),
                input_schema: schema_for::<CrateNameInput>(),
            },
            Tool {
                name: Cow::Borrowed("get_crate_dependencies"),
                description: Cow::Borrowed(
                    "Get the dependencies of a specific crate version. Returns both required and optional dependencies with their version requirements.",
                ),
                input_schema: schema_for::<CrateDependenciesInput>(),
            },
            Tool {
                name: Cow::Borrowed("get_crate_documentation"),
                description: Cow::Borrowed(
                    "Get the documentation URL for a crate. Returns the official docs.rs link and any custom documentation URL specified by the crate author.",
                ),
                input_schema: schema_for::<CrateNameInput>(),
            },
        ]
    }

    async fn search_crates(&self, input: SearchCratesInput) -> CallToolResult {
        let limit = input.limit.clamp(1, 100);
        let query = CratesQuery::builder()
            .search(&input.query)
            .page_size(limit as u64)
            .sort(Sort::Relevance)
            .build();

        match self.client.crates(query).await {
            Ok(response) => {
                let results: Vec<String> = response
                    .crates
                    .iter()
                    .map(|c| {
                        format!(
                            "**{}** (v{}) - {} downloads\n  {}",
                            c.name,
                            c.max_version,
                            c.downloads,
                            c.description.as_deref().unwrap_or("No description")
                        )
                    })
                    .collect();

                if results.is_empty() {
                    CallToolResult::success(vec![Content::text(format!(
                        "No crates found matching '{}'",
                        input.query
                    ))])
                } else {
                    CallToolResult::success(vec![Content::text(format!(
                        "Found {} crates matching '{}':\n\n{}",
                        results.len(),
                        input.query,
                        results.join("\n\n")
                    ))])
                }
            }
            Err(e) => CallToolResult::error(vec![Content::text(format!(
                "Error searching crates: {}",
                e
            ))]),
        }
    }

    async fn get_crate_info(&self, input: CrateNameInput) -> CallToolResult {
        match self.client.get_crate(&input.name).await {
            Ok(response) => {
                let c = &response.crate_data;
                let mut info = format!(
                    "# {}\n\n**Description:** {}\n\n**Latest Version:** {}\n**Total Downloads:** {}\n**Recent Downloads:** {}",
                    c.name,
                    c.description.as_deref().unwrap_or("No description"),
                    c.max_version,
                    c.downloads,
                    c.recent_downloads.unwrap_or(0)
                );

                if let Some(homepage) = &c.homepage {
                    info.push_str(&format!("\n**Homepage:** {}", homepage));
                }
                if let Some(repo) = &c.repository {
                    info.push_str(&format!("\n**Repository:** {}", repo));
                }
                if let Some(docs) = &c.documentation {
                    info.push_str(&format!("\n**Documentation:** {}", docs));
                } else {
                    info.push_str(&format!("\n**Documentation:** https://docs.rs/{}", c.name));
                }

                if let Some(keywords) = &c.keywords
                    && !keywords.is_empty()
                {
                    info.push_str(&format!("\n**Keywords:** {}", keywords.join(", ")));
                }

                if let Some(categories) = &c.categories
                    && !categories.is_empty()
                {
                    info.push_str(&format!("\n**Categories:** {}", categories.join(", ")));
                }

                CallToolResult::success(vec![Content::text(info)])
            }
            Err(crates_io_api::Error::NotFound(_)) => CallToolResult::error(vec![Content::text(
                format!("Crate '{}' not found on crates.io", input.name),
            )]),
            Err(e) => CallToolResult::error(vec![Content::text(format!(
                "Error fetching crate info: {}",
                e
            ))]),
        }
    }

    async fn get_crate_versions(&self, input: CrateNameInput) -> CallToolResult {
        match self.client.get_crate(&input.name).await {
            Ok(response) => {
                let versions: Vec<String> = response
                    .versions
                    .iter()
                    .take(20)
                    .map(|v| {
                        let yanked = if v.yanked { " [YANKED]" } else { "" };
                        format!(
                            "- **v{}**{} - {} downloads (published: {})",
                            v.num,
                            yanked,
                            v.downloads,
                            v.created_at.format("%Y-%m-%d")
                        )
                    })
                    .collect();

                let total = response.versions.len();
                let shown = versions.len();
                let mut result = format!(
                    "# Versions of {}\n\nShowing {} of {} versions:\n\n{}",
                    input.name,
                    shown,
                    total,
                    versions.join("\n")
                );

                if total > shown {
                    result.push_str(&format!(
                        "\n\n*({} more versions not shown)*",
                        total - shown
                    ));
                }

                CallToolResult::success(vec![Content::text(result)])
            }
            Err(crates_io_api::Error::NotFound(_)) => CallToolResult::error(vec![Content::text(
                format!("Crate '{}' not found on crates.io", input.name),
            )]),
            Err(e) => CallToolResult::error(vec![Content::text(format!(
                "Error fetching crate versions: {}",
                e
            ))]),
        }
    }

    async fn get_crate_dependencies(&self, input: CrateDependenciesInput) -> CallToolResult {
        match self
            .client
            .crate_dependencies(&input.name, &input.version)
            .await
        {
            Ok(deps) => {
                if deps.is_empty() {
                    return CallToolResult::success(vec![Content::text(format!(
                        "{} v{} has no dependencies",
                        input.name, input.version
                    ))]);
                }

                let mut normal_deps: Vec<String> = Vec::new();
                let mut dev_deps: Vec<String> = Vec::new();
                let mut build_deps: Vec<String> = Vec::new();

                for dep in &deps {
                    let optional = if dep.optional { " (optional)" } else { "" };
                    let entry = format!("- **{}** {}{}", dep.crate_id, dep.req, optional);

                    match dep.kind.as_str() {
                        "dev" => dev_deps.push(entry),
                        "build" => build_deps.push(entry),
                        _ => normal_deps.push(entry),
                    }
                }

                let mut result = format!("# Dependencies for {} v{}\n", input.name, input.version);

                if !normal_deps.is_empty() {
                    result.push_str(&format!(
                        "\n## Dependencies ({})\n{}\n",
                        normal_deps.len(),
                        normal_deps.join("\n")
                    ));
                }

                if !dev_deps.is_empty() {
                    result.push_str(&format!(
                        "\n## Dev Dependencies ({})\n{}\n",
                        dev_deps.len(),
                        dev_deps.join("\n")
                    ));
                }

                if !build_deps.is_empty() {
                    result.push_str(&format!(
                        "\n## Build Dependencies ({})\n{}\n",
                        build_deps.len(),
                        build_deps.join("\n")
                    ));
                }

                CallToolResult::success(vec![Content::text(result)])
            }
            Err(crates_io_api::Error::NotFound(_)) => {
                CallToolResult::error(vec![Content::text(format!(
                    "Crate '{}' version '{}' not found on crates.io",
                    input.name, input.version
                ))])
            }
            Err(e) => CallToolResult::error(vec![Content::text(format!(
                "Error fetching dependencies: {}",
                e
            ))]),
        }
    }

    async fn get_crate_documentation(&self, input: CrateNameInput) -> CallToolResult {
        match self.client.get_crate(&input.name).await {
            Ok(response) => {
                let c = &response.crate_data;
                let docs_rs = format!("https://docs.rs/{}", c.name);
                let latest_docs = format!("https://docs.rs/{}/{}", c.name, c.max_version);

                let mut result = format!(
                    "# Documentation for {}\n\n**docs.rs:** {}\n**Latest version docs:** {}",
                    c.name, docs_rs, latest_docs
                );

                if let Some(custom_docs) = &c.documentation
                    && custom_docs != &docs_rs
                {
                    result.push_str(&format!("\n**Custom documentation:** {}", custom_docs));
                }

                if let Some(homepage) = &c.homepage {
                    result.push_str(&format!("\n**Homepage:** {}", homepage));
                }

                if let Some(repo) = &c.repository {
                    result.push_str(&format!("\n**Repository:** {}", repo));
                }

                CallToolResult::success(vec![Content::text(result)])
            }
            Err(crates_io_api::Error::NotFound(_)) => CallToolResult::error(vec![Content::text(
                format!("Crate '{}' not found on crates.io", input.name),
            )]),
            Err(e) => CallToolResult::error(vec![Content::text(format!(
                "Error fetching documentation info: {}",
                e
            ))]),
        }
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
        _request: PaginatedRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        async {
            Ok(ListToolsResult {
                tools: Self::tools(),
                next_cursor: None,
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
                    self.search_crates(input).await
                }
                "get_crate_info" => {
                    let input: CrateNameInput =
                        serde_json::from_value(serde_json::Value::Object(args)).map_err(|e| {
                            McpError::invalid_params(format!("Invalid parameters: {}", e), None)
                        })?;
                    self.get_crate_info(input).await
                }
                "get_crate_versions" => {
                    let input: CrateNameInput =
                        serde_json::from_value(serde_json::Value::Object(args)).map_err(|e| {
                            McpError::invalid_params(format!("Invalid parameters: {}", e), None)
                        })?;
                    self.get_crate_versions(input).await
                }
                "get_crate_dependencies" => {
                    let input: CrateDependenciesInput =
                        serde_json::from_value(serde_json::Value::Object(args)).map_err(|e| {
                            McpError::invalid_params(format!("Invalid parameters: {}", e), None)
                        })?;
                    self.get_crate_dependencies(input).await
                }
                "get_crate_documentation" => {
                    let input: CrateNameInput =
                        serde_json::from_value(serde_json::Value::Object(args)).map_err(|e| {
                            McpError::invalid_params(format!("Invalid parameters: {}", e), None)
                        })?;
                    self.get_crate_documentation(input).await
                }
                _ => {
                    return Err(McpError::invalid_params(
                        format!("Unknown tool: {}", name),
                        None,
                    ));
                }
            };

            Ok(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::ServerHandler;
    use serde_json::json;

    #[test]
    fn test_tools_list() {
        let tools = CratesIoServer::tools();
        assert_eq!(tools.len(), 5);

        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert!(tool_names.contains(&"search_crates"));
        assert!(tool_names.contains(&"get_crate_info"));
        assert!(tool_names.contains(&"get_crate_versions"));
        assert!(tool_names.contains(&"get_crate_dependencies"));
        assert!(tool_names.contains(&"get_crate_documentation"));
    }

    #[test]
    fn test_search_crates_schema() {
        let schema = schema_for::<SearchCratesInput>();
        let schema_value = serde_json::Value::Object((*schema).clone());

        // Verify required properties
        let properties = schema_value
            .get("properties")
            .expect("should have properties");
        assert!(
            properties.get("query").is_some(),
            "should have query property"
        );
        assert!(
            properties.get("limit").is_some(),
            "should have limit property"
        );
    }

    #[test]
    fn test_crate_name_input_deserialization() {
        let input: CrateNameInput = serde_json::from_value(json!({
            "name": "serde"
        }))
        .expect("should deserialize");

        assert_eq!(input.name, "serde");
    }

    #[test]
    fn test_crate_dependencies_input_deserialization() {
        let input: CrateDependenciesInput = serde_json::from_value(json!({
            "name": "tokio",
            "version": "1.0.0"
        }))
        .expect("should deserialize");

        assert_eq!(input.name, "tokio");
        assert_eq!(input.version, "1.0.0");
    }

    #[test]
    fn test_search_crates_input_default_limit() {
        let input: SearchCratesInput = serde_json::from_value(json!({
            "query": "async"
        }))
        .expect("should deserialize");

        assert_eq!(input.query, "async");
        assert_eq!(input.limit, 10); // default value
    }

    #[test]
    fn test_search_crates_input_custom_limit() {
        let input: SearchCratesInput = serde_json::from_value(json!({
            "query": "async",
            "limit": 50
        }))
        .expect("should deserialize");

        assert_eq!(input.limit, 50);
    }

    #[test]
    fn test_limit_clamping() {
        // Verify clamp logic: limit should be between 1 and 100
        assert_eq!(0_u32.clamp(1, 100), 1);
        assert_eq!(50_u32.clamp(1, 100), 50);
        assert_eq!(100_u32.clamp(1, 100), 100);
        assert_eq!(150_u32.clamp(1, 100), 100);
    }

    #[test]
    fn test_server_info() {
        let server = CratesIoServer::new().expect("should create server");
        let info = server.get_info();

        assert!(info.instructions.is_some());
        let instructions = info.instructions.unwrap();
        assert!(instructions.contains("crates.io"));
    }

    #[test]
    fn test_tool_descriptions_not_empty() {
        let tools = CratesIoServer::tools();
        for tool in tools {
            assert!(
                !tool.description.is_empty(),
                "Tool {} should have description",
                tool.name
            );
            assert!(!tool.name.is_empty(), "Tool name should not be empty");
        }
    }

    #[tokio::test]
    async fn test_server_creation() {
        let server = CratesIoServer::new();
        assert!(server.is_ok(), "Server should be created successfully");
    }
}
