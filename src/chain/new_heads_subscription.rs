use crate::chain::new_heads_subscription::BlobSplitter::BlobSegmentPosted;
use alloy_primitives::Address;
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::{Filter, Header};
use alloy_sol_types::{sol, SolEvent};
use alloy_transport_ws::WsConnect;
use futures::StreamExt;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::SendTimeoutError;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

// Codegen from ABI file to interact with the contract.
sol!(BlobSplitter, "src/abi/BlobSplitter.json");

pub type HeaderWithLogs = (Header, Vec<BlobSegmentPosted>);

/// New heads subscription, so we know when to begin a new aggregation job
#[derive(Debug)]
pub struct NewHeadsSubscription {
    /// WebSocket URL to connect to the provider
    ws_url: String,
}

impl NewHeadsSubscription {
    /// Creates a new `NewHeads` instance
    ///
    /// # Arguments
    ///
    /// * `ws_url` - WebSocket URL to connect to the provider
    pub fn new(ws_url: String) -> Self {
        Self { ws_url }
    }

    /// Subscribe to new block headers, so we know when to begin a new aggregation job
    pub async fn subscribe_to_new_heads(
        &self,
        input: mpsc::Sender<HeaderWithLogs>,
        blob_splitter_address: Address,
        cancel: CancellationToken,
    ) -> eyre::Result<JoinHandle<()>> {
        // Create provider
        let provider = ProviderBuilder::new()
            .on_ws(WsConnect::new(&self.ws_url))
            .await?;

        // Subscribe to new block heads
        let handle = tokio::spawn(async move {
            // Subscription must be defined within task async task, because of its lifetime constraints.
            let subscription = match provider.subscribe_blocks().await {
                Ok(sub) => sub,
                Err(e) => {
                    error!("Failed to subscribe to newHeads: {:?}", e);
                    return;
                }
            };

            let mut stream = subscription.into_stream();
            let timeout = Duration::from_millis(500);
            loop {
                // Stop listening when global cancellation is triggered.
                if cancel.is_cancelled() {
                    break;
                }

                // Wait for new block heads
                if let Some(header) = stream.next().await {
                    debug!("Received new head: {:?}", header);

                    let log_filter = Filter::new()
                        .at_block_hash(header.hash)
                        .address(blob_splitter_address)
                        .event_signature(BlobSegmentPosted::SIGNATURE_HASH);
                    let logs = provider.get_logs(&log_filter).await.unwrap_or_default();
                    let decoded_logs: Vec<BlobSegmentPosted> = logs
                        .iter()
                        .map(|i| BlobSegmentPosted::decode_log_data(i.data(), true).unwrap())
                        .collect();

                    match input.send_timeout((header, decoded_logs), timeout).await {
                        Ok(()) => {}
                        Err(SendTimeoutError::Timeout(_)) => {
                            // Channel is most likely full, when the timeout elapsed
                            warn!("Failed to send block header, timeout")
                        }
                        Err(SendTimeoutError::Closed(_)) => {}
                    }
                }
            }
        });

        info!("Subscribed to new heads");

        Ok(handle)
    }
}
