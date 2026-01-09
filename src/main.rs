use anyhow::Result;
use cratesdotio_mcp::CratesIoServer;
use rmcp::ServiceExt;
use rmcp::transport::stdio;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::{
    StreamableHttpServerConfig, StreamableHttpService,
};
use std::env;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn print_usage() {
    eprintln!("Usage: cratesdotio-mcp [OPTIONS]");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --http [PORT]  Start HTTP/SSE server (default port: 3000)");
    eprintln!("  --help         Show this help message");
    eprintln!();
    eprintln!("Without --http, the server uses stdio transport for MCP clients.");
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    // Check for help
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_usage();
        return Ok(());
    }

    // Initialize logging to stderr (stdout is used for MCP communication in stdio mode)
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();

    // Check for --http flag and optional port argument
    let http_index = args.iter().position(|a| a == "--http");

    if let Some(idx) = http_index {
        // Get port from next argument, or default to 3000
        let port = args
            .get(idx + 1)
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(3000);

        // Bind to IPv6 Unspecified (::) to allow dual-stack (IPv4 and IPv6) connections
        let bind_addr = std::net::SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port));

        tracing::info!(
            "Starting crates.io MCP server with HTTP/SSE transport on {}",
            bind_addr
        );

        let server_factory =
            || CratesIoServer::new().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
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
        tracing::info!("Starting crates.io MCP server with stdio transport");

        let server = CratesIoServer::new()?;
        let service = server.serve(stdio()).await?;

        tracing::info!("Server running, waiting for requests...");
        service.waiting().await?;
    }

    tracing::info!("Server shutting down");
    Ok(())
}
