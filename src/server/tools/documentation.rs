use super::info::CrateNameInput;
use super::schema_for;
use anyhow::Result;
use crates_io_api::AsyncClient;
use rmcp::model::{CallToolResult, Content, Tool};

pub struct GetCrateDocumentationTool {
    pub inner: Tool,
}

impl Default for GetCrateDocumentationTool {
    fn default() -> Self {
        Self {
            inner: Tool::new(Self::NAME, Self::DESC, schema_for::<CrateNameInput>()),
        }
    }
}

impl GetCrateDocumentationTool {
    pub const NAME: &'static str = "get_crate_documentation";
    const DESC: &'static str = "Get the documentation URL for a crate. Returns the official docs.rs link and any custom documentation URL specified by the crate author.";

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

                Ok(CallToolResult::success(vec![Content::text(result)]))
            }
            Err(crates_io_api::Error::NotFound(_)) => {
                Ok(CallToolResult::error(vec![Content::text(format!(
                    "Crate '{}' not found on crates.io",
                    input.name
                ))]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error fetching documentation info: {}",
                e
            ))])),
        }
    }
}
