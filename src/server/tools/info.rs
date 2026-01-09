use super::schema_for;
use anyhow::Result;
use crates_io_api::AsyncClient;
use rmcp::model::{CallToolResult, Content, Tool};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CrateNameInput {
    /// Name of the crate
    pub name: String,
}

pub struct GetCrateInfoTool {
    pub inner: Tool,
}

impl Default for GetCrateInfoTool {
    fn default() -> Self {
        Self {
            inner: Tool::new(Self::NAME, Self::DESC, schema_for::<CrateNameInput>()),
        }
    }
}

impl GetCrateInfoTool {
    pub const NAME: &'static str = "get_crate_info";
    const DESC: &'static str = "Get detailed information about a specific crate including description, homepage, repository, downloads, and latest version.";

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

                Ok(CallToolResult::success(vec![Content::text(info)]))
            }
            Err(crates_io_api::Error::NotFound(_)) => {
                Ok(CallToolResult::error(vec![Content::text(format!(
                    "Crate '{}' not found on crates.io",
                    input.name
                ))]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error fetching crate info: {}",
                e
            ))])),
        }
    }
}
