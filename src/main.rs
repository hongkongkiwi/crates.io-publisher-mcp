use anyhow::Result;
use clap::Parser;
use cratesdotio_mcp::CratesIoServer;
use rmcp::ServiceExt;
use rmcp::transport::stdio;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::{
    StreamableHttpServerConfig, StreamableHttpService,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Start HTTP/SSE server on the specified port.
    /// If no port is specified, it defaults to 3000.
    #[arg(long, value_name = "PORT", num_args = 0..=1, default_missing_value = "3000")]
    http: Option<u16>,

    /// Start HTTP/SSE server on PORT (alternative flag style if user wants just --http without value, handling might be tricky with Option<u16>,
    /// better to use a flag and a separate port option, OR use `num_args` and `default_missing_value` as above.
    /// The user seemingly wants `--http [PORT]` where PORT is optional.
    /// `clap` handles optional values for flags with `num_args`.
    /// However, usually `--http 8080` or just `--http` (implying default)
    /// The previous manual parsing was `--http [PORT]` checking next arg.
    /// `default_missing_value` works if the flag is present but no value is provided.
    /// If the flag is NOT present, `http` will be None.
    /// This matches the previous `if let Some(idx) = http_index` logic.
    #[arg(skip)]
    _dummy: (), // field to allow comments/attributes above to not look weird
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse args
    let cli = Cli::parse();

    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();

    if let Some(port) = cli.http {
        // HTTP Mode
        // Bind to IPv6 Unspecified (::) for dual-stack
        let bind_addr = std::net::SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port));

        tracing::info!(
            "Starting crates.io MCP server with HTTP/SSE transport on {}",
            bind_addr
        );

        let server_factory = || CratesIoServer::new().map_err(std::io::Error::other);
        let session_manager = std::sync::Arc::new(LocalSessionManager::default());
        let config = StreamableHttpServerConfig::default();

        let service = StreamableHttpService::new(server_factory, session_manager, config);

        let app = axum::Router::new().fallback_service(service);

        let listener = tokio::net::TcpListener::bind(bind_addr).await?;

        tracing::info!("Server running on http://localhost:{}", port);
        tracing::info!("  SSE endpoint: http://localhost:{}/sse", port);
        tracing::info!("  POST endpoint: http://localhost:{}/message", port);

        axum::serve(listener, app).await?;
    } else {
        // Stdio Mode
        tracing::info!("Starting crates.io MCP server with stdio transport");

        let server = CratesIoServer::new()?;
        let service = server.serve(stdio()).await?;

        tracing::info!("Server running, waiting for requests...");
        service.waiting().await?;
    }

    tracing::info!("Server shutting down");
    Ok(())
}
