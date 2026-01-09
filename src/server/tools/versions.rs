use super::info::CrateNameInput;
use super::schema_for;
use anyhow::Result;
use crates_io_api::AsyncClient;
use rmcp::model::{CallToolResult, Content, Tool};

pub struct GetCrateVersionsTool {
    pub inner: Tool,
}

impl Default for GetCrateVersionsTool {
    fn default() -> Self {
        Self {
            inner: Tool::new(Self::NAME, Self::DESC, schema_for::<CrateNameInput>()),
        }
    }
}

impl GetCrateVersionsTool {
    pub const NAME: &'static str = "get_crate_versions";
    const DESC: &'static str = "Get a list of all published versions of a crate, including version numbers, publication dates, and download counts.";

    #[deprecated(since = "0.1.0", note = "Use Default::default() instead")]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn execute(
        &self,
        client: &AsyncClient,
        input: CrateNameInput,
    ) -> Result<CallToolResult> {
        match client.get_crate(&input.name).await {
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

                Ok(CallToolResult::success(vec![Content::text(result)]))
            }
            Err(crates_io_api::Error::NotFound(_)) => {
                Ok(CallToolResult::error(vec![Content::text(format!(
                    "Crate '{}' not found on crates.io",
                    input.name
                ))]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error fetching crate versions: {}",
                e
            ))])),
        }
    }
}
