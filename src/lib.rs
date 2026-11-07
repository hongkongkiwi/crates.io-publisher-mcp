pub mod api;
pub mod auth;
pub mod mode;
pub mod ops;
pub mod server;

pub use auth::{Token, TokenSource, resolve_token};
pub use mode::Mode;
pub use server::{CratesIoServer, ServerConfig};

pub const USER_AGENT: &str = concat!(
    "cratesdotio-mcp/",
    env!("CARGO_PKG_VERSION"),
    " (MCP server for crates.io)"
);

pub const DEFAULT_BASE_URL: &str = "https://crates.io";
