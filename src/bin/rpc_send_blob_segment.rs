use alloy_primitives::hex::ToHexExt;
use alloy_primitives::{keccak256, Bytes, U128};
use blobsy_aggregator::rpc::serialize::RawBlobSegment;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::HttpClientBuilder;
use std::any::Any;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::time::sleep;
use tracing::{debug, error};

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Initialize tracing subscriber
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    // Initialize RPC client
    let blobsy_aggregator_rpc = "http://localhost:9645";
    let client = HttpClientBuilder::default().build(blobsy_aggregator_rpc)?;

    // Crate and send new blob segment every [timeout] seconds
    let timeout = Duration::from_secs(12);
    loop {
        /// The [epoch_timestamp] ensures each blob segment is unique.
        /// To send duplicate segments, use a fixed timestamp value instead of the current time.
        let now = SystemTime::now();
        let epoch_timestamp = now.duration_since(UNIX_EPOCH)?.as_secs();
        let data = format!("RPC_SEND_BLOB_SEGMENT [timestamp: {}]", epoch_timestamp).encode_hex();

        // Create new blob segment
        let blob_segment = RawBlobSegment {
            block_number: None,
            max_blob_segment_fee: U128::from(epoch_timestamp),
            blob_segment_data: Bytes::from(data),
            callback_contract: None,
            callback_payload: None,
            signing_address: None,
            min_timestamp: None,
            max_timestamp: None,
            first_seen_at: None,
        };

        // Send new blob segment
        match client
            .request::<(), [RawBlobSegment; 1]>("eth_sendBlobSegment", [blob_segment.clone()])
            .await
        {
            Ok(_) => debug!("Sent blob segment: {:?}", blob_segment),
            Err(e) => error!("Error sending blob segment: {}", e),
        }

        sleep(timeout).await;
    }
}
