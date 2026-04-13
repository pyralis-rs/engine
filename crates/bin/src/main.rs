//! Binary entry point that wires ingestion streams and runtime lifecycle.

use std::str::FromStr;
use std::sync::Arc;

use alloy::primitives::{Address, U256};
use anyhow::{Context, Result};
use pyralis_core::traits::Simulator;
use pyralis_core::types::{PoolState, SimulationContext};
use pyralis_ingestion::{AlloyProvider, ProviderRegistration, StreamManager};
use pyralis_simulation::SimulationEngine;
use pyralis_strategy::{StrategyRegistry, TwoPoolArbStrategy};
use serde::Deserialize;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

/// Runtime configuration loaded from TOML.
#[derive(Debug, Deserialize)]
struct AppConfig {
    network: NetworkConfig,
    strategy: StrategyConfig,
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

/// Strategy configuration fields.
#[derive(Debug, Deserialize)]
struct StrategyConfig {
    min_profit_threshold: String,
    max_gas_price: String,
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
    let pool_manager_address = app_config
        .network
        .pool_manager_address
        .parse::<Address>()
        .context("invalid pool_manager_address in config/default.toml")?;
    let min_profit_threshold = parse_u256(&app_config.strategy.min_profit_threshold)
        .context("invalid strategy.min_profit_threshold")?;
    let max_gas_price =
        parse_u256(&app_config.strategy.max_gas_price).context("invalid strategy.max_gas_price")?;
    let estimated_gas_cost_wei = max_gas_price.saturating_mul(U256::from(250_000_u64));

    println!("Pyralis Engine starting...");
    info!(
        chain_id = app_config.network.chain_id,
        chain_name = %app_config.network.chain_name,
        ws_endpoints = app_config.network.rpc_ws_endpoints.len(),
        http_endpoints = app_config.network.rpc_http_endpoints.len(),
        pool_manager_address = %app_config.network.pool_manager_address,
        block_time_ms = app_config.network.block_time_ms,
        min_profit_threshold = %app_config.strategy.min_profit_threshold,
        max_gas_price = %app_config.strategy.max_gas_price,
        "loaded configuration"
    );

    let providers = build_providers(&app_config.network.rpc_ws_endpoints).await?;
    let simulation_provider = providers
        .first()
        .map(|entry| Arc::clone(&entry.provider))
        .context("no provider available for simulation engine")?;
    let simulation_engine = SimulationEngine::new(simulation_provider);

    let mut strategy_registry = StrategyRegistry::new();
    strategy_registry.register(TwoPoolArbStrategy::new(
        50,
        min_profit_threshold,
        U256::from(1_000_000_000_000_000_000_u64),
        estimated_gas_cost_wei,
    ));
    info!(
        registered_strategies = ?strategy_registry.list_strategies(),
        "strategy registry initialized"
    );

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

                        if let Err(error) = simulation_engine.load_state(block.number).await {
                            warn!(block_number = block.number, error = %error, "failed to load simulation state");
                            continue;
                        }

                        let context = SimulationContext {
                            block: block.clone(),
                            pool_states: build_pool_states(pool_manager_address, block.number),
                            pending_txs: Vec::new(),
                        };
                        let opportunities = strategy_registry.evaluate_all(&context).await;
                        if opportunities.is_empty() {
                            info!(block_number = block.number, "no opportunities found");
                        } else {
                            info!(
                                block_number = block.number,
                                count = opportunities.len(),
                                best_profit = %opportunities[0].estimated_profit,
                                "opportunities found"
                            );
                        }
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

fn parse_u256(value: &str) -> Result<U256> {
    U256::from_str(value).map_err(|error| anyhow::anyhow!("invalid U256 value {value}: {error}"))
}

fn build_pool_states(pool_manager_address: Address, block_number: u64) -> Vec<PoolState> {
    let q96: U256 = U256::from(1_u128) << 96;
    let skew_bps = U256::from((block_number % 200) + 40);
    let skew = q96.saturating_mul(skew_bps) / U256::from(10_000_u64);

    vec![
        PoolState {
            address: pool_manager_address,
            token0: Address::repeat_byte(0xAA),
            token1: Address::repeat_byte(0xBB),
            fee: 500,
            sqrt_price_x96: q96,
            liquidity: 1_000_000,
            tick: 0,
        },
        PoolState {
            address: pool_manager_address,
            token0: Address::repeat_byte(0xAA),
            token1: Address::repeat_byte(0xBB),
            fee: 3_000,
            sqrt_price_x96: q96.saturating_add(skew),
            liquidity: 2_000_000,
            tick: 5,
        },
    ]
}
