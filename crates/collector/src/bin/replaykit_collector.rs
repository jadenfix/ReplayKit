use std::net::SocketAddr;
use std::path::PathBuf;

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let port: u16 = std::env::var("REPLAYKIT_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4100);

    let data_root = std::env::var("REPLAYKIT_DATA_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./data"));

    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    replaykit_collector::server::serve(addr, data_root).await
}
