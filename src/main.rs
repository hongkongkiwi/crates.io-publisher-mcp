use anyhow::Result;
use cratesdotio_mcp::CratesIoServer;
use rmcp::ServiceExt;
use rmcp::transport::sse_server::SseServer;
use rmcp::transport::stdio;
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

        let bind_addr: std::net::SocketAddr = format!("0.0.0.0:{}", port).parse()?;

        tracing::info!(
            "Starting crates.io MCP server with HTTP/SSE transport on {}",
            bind_addr
        );

        let sse_server = SseServer::serve(bind_addr).await?;

        // with_service takes a factory that creates a new server for each connection
        let ct =
            sse_server.with_service(|| CratesIoServer::new().expect("Failed to create server"));

        tracing::info!("Server running on http://localhost:{}", port);
        tracing::info!("  SSE endpoint: http://localhost:{}/sse", port);
        tracing::info!("  POST endpoint: http://localhost:{}/message", port);

        // Wait for cancellation
        ct.cancelled().await;
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
