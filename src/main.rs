use anyhow::{Result, bail};
use clap::Parser;
use cratesdotio_mcp::{CratesIoServer, Mode, ServerConfig};
use rmcp::ServiceExt;
use rmcp::transport::stdio;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::{
    StreamableHttpServerConfig, StreamableHttpService,
};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Access mode: read-only, write, or publish. Required, with no default.
    #[arg(long, value_enum, env = "CRATES_MCP_MODE")]
    mode: Mode,

    /// Start the HTTP/SSE server on PORT. If no port is given, it defaults to 3000.
    #[arg(long, value_name = "PORT", num_args = 0..=1, default_missing_value = "3000")]
    http: Option<u16>,

    /// Address to bind the HTTP server to. Defaults to all interfaces in read-only
    /// mode and loopback in write and publish modes.
    #[arg(long, value_name = "ADDR")]
    bind: Option<IpAddr>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();

    let mode = cli.mode;
    let config = ServerConfig::new(mode)?;

    if let Some(port) = cli.http {
        let bind = cli.bind.unwrap_or_else(|| {
            if mode == Mode::ReadOnly {
                IpAddr::V6(Ipv6Addr::UNSPECIFIED)
            } else {
                IpAddr::V4(Ipv4Addr::LOCALHOST)
            }
        });

        // A write-capable server reachable from the network holds an open
        // publishing credential. Keep those modes on the loopback interface.
        if mode != Mode::ReadOnly && !bind.is_loopback() {
            bail!(
                "`--mode {mode}` over HTTP must bind to loopback, not {bind}. Use stdio for \
                 write access, or run the HTTP server in read-only mode."
            );
        }

        let address = std::net::SocketAddr::new(bind, port);
        tracing::info!("Starting crates.io MCP server in {mode} mode on {address}");

        let server_config = config.clone();
        let server_factory =
            move || CratesIoServer::new(server_config.clone()).map_err(std::io::Error::other);
        let session_manager = std::sync::Arc::new(LocalSessionManager::default());
        let http_config = StreamableHttpServerConfig::default();

        let service = StreamableHttpService::new(server_factory, session_manager, http_config);
        let app = axum::Router::new().fallback_service(service);

        let listener = tokio::net::TcpListener::bind(address).await?;
        tracing::info!("Server running on http://{address}");

        axum::serve(listener, app).await?;
    } else {
        tracing::info!("Starting crates.io MCP server in {mode} mode over stdio");

        let server = CratesIoServer::new(config)?;
        let service = server.serve(stdio()).await?;

        tracing::info!("Server running, waiting for requests...");
        service.waiting().await?;
    }

    tracing::info!("Server shutting down");
    Ok(())
}
