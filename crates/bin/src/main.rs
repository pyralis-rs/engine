//! Binary entry point that wires ingestion streams and runtime lifecycle.

use std::sync::Arc;

use anyhow::{Context, Result};
use pyralis_ingestion::{AlloyProvider, ProviderRegistration, StreamManager};
use serde::Deserialize;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

/// Runtime configuration loaded from TOML.
#[derive(Debug, Deserialize)]
struct AppConfig {
    network: NetworkConfig,
    observability: ObservabilityConfig,
}

/// Network-facing configuration fields.
#[derive(Debug, Deserialize)]
struct NetworkConfig {
    chain_id: u64,
    chain_name: String,
    rpc_ws_endpoints: Vec<String>,
    rpc_http_endpoints: Vec<String>,
    pool_manager_address: String,
    block_time_ms: u64,
}

/// Logging and telemetry configuration fields.
#[derive(Debug, Deserialize)]
struct ObservabilityConfig {
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let app_config = load_config()?;
    init_tracing(&app_config.observability.log_level)?;

    println!("Pyralis Engine starting...");
    info!(
        chain_id = app_config.network.chain_id,
        chain_name = %app_config.network.chain_name,
        ws_endpoints = app_config.network.rpc_ws_endpoints.len(),
        http_endpoints = app_config.network.rpc_http_endpoints.len(),
        pool_manager_address = %app_config.network.pool_manager_address,
        block_time_ms = app_config.network.block_time_ms,
        "loaded configuration"
    );

    let providers = build_providers(&app_config.network.rpc_ws_endpoints).await?;
    let mut stream_manager = StreamManager::new(providers);
    stream_manager.start().await?;

    let mut block_receiver = stream_manager.subscribe_blocks();
    loop {
        tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                if signal.is_ok() {
                    info!("ctrl+c received, shutting down");
                } else {
                    warn!("failed to listen for ctrl+c, shutting down");
                }
                break;
            }
            message = block_receiver.recv() => {
                match message {
                    Ok(block) => {
                        println!("new block: {} {}", block.number, block.hash);
                        info!(block_number = block.number, block_hash = %block.hash, "new block");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(skipped, "block receiver lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        warn!("block stream closed");
                        break;
                    }
                }
            }
        }
    }

    stream_manager.shutdown().await;
    info!("shutdown complete");
    Ok(())
}

fn load_config() -> Result<AppConfig> {
    let settings = config::Config::builder()
        .add_source(config::File::with_name("config/default.toml"))
        .build()
        .context("failed to load config/default.toml")?;

    settings
        .try_deserialize::<AppConfig>()
        .context("failed to deserialize application config")
}

fn init_tracing(log_level: &str) -> Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(log_level))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .try_init()
        .map_err(|error| anyhow::anyhow!("failed to initialize tracing subscriber: {error}"))?;

    Ok(())
}

async fn build_providers(
    ws_endpoints: &[String],
) -> Result<Vec<ProviderRegistration<AlloyProvider>>> {
    let mut providers = Vec::new();

    for endpoint in ws_endpoints {
        match AlloyProvider::new(vec![endpoint.clone()]).await {
            Ok(provider) => {
                providers.push(ProviderRegistration::new(
                    endpoint.clone(),
                    Arc::new(provider),
                ));
            }
            Err(error) => {
                warn!(endpoint = %endpoint, error = %error, "provider initialization failed");
            }
        }
    }

    if providers.is_empty() {
        anyhow::bail!("no provider could be initialized from configured WebSocket endpoints");
    }

    Ok(providers)
}
