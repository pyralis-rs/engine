//! Alloy-backed chain provider implementation.

use alloy::consensus::BlockHeader;
use alloy::eips::BlockNumberOrTag;
use alloy::primitives::{Address, B256, U256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder, WsConnect};
use alloy::rpc::types::eth::{Block, Header, Transaction};
use pyralis_core::error::{PyralisError, Result};
use pyralis_core::traits::ChainDataProvider;
use pyralis_core::types::BlockInfo;
use tokio::sync::broadcast;

const DEFAULT_BLOCK_CHANNEL_SIZE: usize = 1024;
const DEFAULT_PENDING_CHANNEL_SIZE: usize = 4096;

/// Chain data provider backed by Alloy WebSocket subscriptions.
#[derive(Clone, Debug)]
pub struct AlloyProvider {
    ws_endpoints: Vec<String>,
    active_endpoint: String,
    provider: DynProvider,
}

impl AlloyProvider {
    /// Creates a new provider and connects to the first healthy endpoint.
    pub async fn new(ws_endpoints: Vec<String>) -> Result<Self> {
        if ws_endpoints.is_empty() {
            return Err(PyralisError::Config(
                "at least one WebSocket endpoint must be configured".to_string(),
            ));
        }

        let mut last_error = None;
        for endpoint in ws_endpoints.iter().cloned() {
            match Self::connect(&endpoint).await {
                Ok(provider) => {
                    return Ok(Self {
                        ws_endpoints,
                        active_endpoint: endpoint,
                        provider,
                    });
                }
                Err(error) => last_error = Some(error),
            }
        }

        let error = last_error
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown provider connection error".to_string());
        Err(PyralisError::Provider(format!(
            "unable to connect to any configured endpoint: {error}"
        )))
    }

    /// Returns the endpoint used by the active Alloy connection.
    pub fn active_endpoint(&self) -> &str {
        &self.active_endpoint
    }

    /// Returns all configured WebSocket endpoints.
    pub fn ws_endpoints(&self) -> &[String] {
        &self.ws_endpoints
    }

    async fn connect(endpoint: &str) -> Result<DynProvider> {
        let provider = ProviderBuilder::new()
            .connect_ws(WsConnect::new(endpoint))
            .await?;
        Ok(provider.erased())
    }
}

#[allow(async_fn_in_trait)]
impl ChainDataProvider for AlloyProvider {
    async fn subscribe_blocks(&self) -> Result<broadcast::Receiver<BlockInfo>> {
        let mut subscription = self.provider.subscribe_blocks().await?;
        let (sender, receiver) = broadcast::channel(DEFAULT_BLOCK_CHANNEL_SIZE);

        tokio::spawn(async move {
            loop {
                match subscription.recv().await {
                    Ok(header) => {
                        let block = block_header_to_info(&header);
                        if sender.send(block).is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        Ok(receiver)
    }

    async fn subscribe_pending_txs(&self) -> Result<broadcast::Receiver<Transaction>> {
        let mut subscription = self.provider.subscribe_full_pending_transactions().await?;
        let (sender, receiver) = broadcast::channel(DEFAULT_PENDING_CHANNEL_SIZE);

        tokio::spawn(async move {
            loop {
                match subscription.recv().await {
                    Ok(tx) => {
                        if sender.send(tx).is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        Ok(receiver)
    }

    async fn get_storage_at(
        &self,
        address: Address,
        slot: B256,
        block_number: Option<u64>,
    ) -> Result<B256> {
        let slot = U256::from_be_slice(slot.as_slice());
        let mut call = self.provider.get_storage_at(address, slot);

        if let Some(number) = block_number {
            call = call.number(number);
        }

        let value = call.await?;
        Ok(value.into())
    }

    async fn get_block_by_number(&self, block_number: u64) -> Result<Option<BlockInfo>> {
        let block = self
            .provider
            .get_block_by_number(BlockNumberOrTag::Number(block_number))
            .await?;
        Ok(block.map(|value| block_to_info(&value)))
    }
}

pub(crate) fn block_header_to_info(header: &Header) -> BlockInfo {
    BlockInfo {
        number: header.number(),
        hash: header.hash,
        timestamp: header.timestamp(),
        base_fee: header.base_fee_per_gas().map(u128::from),
    }
}

pub(crate) fn block_to_info(block: &Block) -> BlockInfo {
    block_header_to_info(&block.header)
}

#[cfg(test)]
mod tests {
    use alloy::consensus::Header as ConsensusHeader;
    use alloy::primitives::B256;

    use super::block_header_to_info;

    #[test]
    fn test_block_info_creation_from_header() {
        let mut inner = ConsensusHeader::default();
        inner.number = 101;
        inner.timestamp = 1_700_000_123;
        inner.base_fee_per_gas = Some(7);

        let header = alloy::rpc::types::eth::Header::new(inner);
        let info = block_header_to_info(&header);

        assert_eq!(info.number, 101);
        assert_eq!(info.timestamp, 1_700_000_123);
        assert_eq!(info.base_fee, Some(7));
        assert_ne!(info.hash, B256::ZERO);
    }

    #[tokio::test]
    async fn test_provider_new_requires_endpoints() {
        let result = super::AlloyProvider::new(Vec::new()).await;
        assert!(result.is_err());
    }
}
