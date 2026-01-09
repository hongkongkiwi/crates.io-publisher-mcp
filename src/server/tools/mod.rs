use schemars::JsonSchema;
use std::sync::Arc;

pub mod dependencies;
pub mod documentation;
pub mod info;
pub mod search;
pub mod versions;

pub use dependencies::GetCrateDependenciesTool;
pub use documentation::GetCrateDocumentationTool;
pub use info::GetCrateInfoTool;
pub use search::SearchCratesTool;
pub use versions::GetCrateVersionsTool;

pub(crate) fn schema_for<T: JsonSchema>() -> Arc<serde_json::Map<String, serde_json::Value>> {
    let schema = schemars::schema_for!(T);
    match serde_json::to_value(schema).unwrap() {
        serde_json::Value::Object(m) => Arc::new(m),
        _ => panic!("schema should be an object"),
    }
}
