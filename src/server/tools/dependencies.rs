use super::schema_for;
use anyhow::Result;
use crates_io_api::AsyncClient;
use rmcp::model::{CallToolResult, Content, Tool};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CrateDependenciesInput {
    /// Name of the crate
    pub name: String,
    /// Version of the crate (e.g., "1.0.0")
    pub version: String,
}

pub struct GetCrateDependenciesTool {
    pub inner: Tool,
}

impl Default for GetCrateDependenciesTool {
    fn default() -> Self {
        Self {
            inner: Tool::new(
                Self::NAME,
                Self::DESC,
                schema_for::<CrateDependenciesInput>(),
            ),
        }
    }
}

impl GetCrateDependenciesTool {
    pub const NAME: &'static str = "get_crate_dependencies";
    const DESC: &'static str = "Get the dependencies of a specific crate version. Returns both required and optional dependencies with their version requirements.";

    #[deprecated(since = "0.1.0", note = "Use Default::default() instead")]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn execute(
        &self,
        client: &AsyncClient,
        input: CrateDependenciesInput,
    ) -> Result<CallToolResult> {
        match client.crate_dependencies(&input.name, &input.version).await {
            Ok(deps) => {
                if deps.is_empty() {
                    return Ok(CallToolResult::success(vec![Content::text(format!(
                        "{} v{} has no dependencies",
                        input.name, input.version
                    ))]));
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

                Ok(CallToolResult::success(vec![Content::text(result)]))
            }
            Err(crates_io_api::Error::NotFound(_)) => {
                Ok(CallToolResult::error(vec![Content::text(format!(
                    "Crate '{}' version '{}' not found on crates.io",
                    input.name, input.version
                ))]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error fetching dependencies: {}",
                e
            ))])),
        }
    }
}
