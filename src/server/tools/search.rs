use super::schema_for;
use anyhow::Result;
use crates_io_api::{AsyncClient, CratesQuery, Sort};
use rmcp::model::{CallToolResult, Content, Tool};
use schemars::JsonSchema;
use serde::Deserialize;

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

pub struct SearchCratesTool {
    pub inner: Tool,
}

impl Default for SearchCratesTool {
    fn default() -> Self {
        Self {
            inner: Tool::new(Self::NAME, Self::DESC, schema_for::<SearchCratesInput>()),
        }
    }
}

impl SearchCratesTool {
    pub const NAME: &'static str = "search_crates";
    const DESC: &'static str = "Search for crates on crates.io by name or keyword. Returns a list of matching crates with their descriptions and download counts.";

    #[deprecated(since = "0.1.0", note = "Use Default::default() instead")]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn execute(
        &self,
        client: &AsyncClient,
        input: SearchCratesInput,
    ) -> Result<CallToolResult> {
        let limit = input.limit.clamp(1, 100);
        let query = CratesQuery::builder()
            .search(&input.query)
            .page_size(limit as u64)
            .sort(Sort::Relevance)
            .build();

        match client.crates(query).await {
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
                    Ok(CallToolResult::success(vec![Content::text(format!(
                        "No crates found matching '{}'",
                        input.query
                    ))]))
                } else {
                    Ok(CallToolResult::success(vec![Content::text(format!(
                        "Found {} crates matching '{}':\n\n{}",
                        results.len(),
                        input.query,
                        results.join("\n\n")
                    ))]))
                }
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error searching crates: {}",
                e
            ))])),
        }
    }
}
