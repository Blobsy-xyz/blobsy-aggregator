use crate::building::blob_aggregator::AggregationContext;
use crate::building::partial_blob::PartialBlob;
use crate::primitives::blob_segment::BlobSegment;
use crate::submission::rpc_submitter::RpcSubmitter;
use alloy_eips::merge::SLOT_DURATION_SECS;
use alloy_primitives::Bytes;
use alloy_rpc_types_eth::Header;
use std::os::unix::raw::ino_t;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::field::debug;
use tracing::{debug, error, info};

/// Aggregator appending blob segments until current blob is full. Once full new blob is created.
pub struct AppendingBlobAggregator {
    ctx: AggregationContext,
    input: broadcast::Receiver<BlobSegment>,
    rpc_submit: Arc<RpcSubmitter>,
    cancellation_token: CancellationToken,
}

impl AppendingBlobAggregator {
    /// Creates a new `AppendingBlobAggregator` instance
    ///
    /// # Arguments
    /// * `ctx` - Aggregation task context
    /// * `input` - BlobSegments receiver channel
    /// * `rpc_submit` - RpcSubmitter for submitting blobs
    /// * `cancellation_token` - Cancellation token for stopping the aggregation
    pub fn new(
        ctx: AggregationContext,
        input: broadcast::Receiver<BlobSegment>,
        rpc_submit: Arc<RpcSubmitter>,
        cancellation_token: CancellationToken,
    ) -> Self {
        info!("Appending Blob Aggregator INITIALIZED");

        Self {
            ctx,
            input,
            rpc_submit,
            cancellation_token,
        }
    }

    /// Build blobs from received blob segments
    pub async fn build_blobs(mut self) {
        info!("New Appending Blob Aggregator STARTED");

        let mut partial_blobs: Vec<PartialBlob> = vec![];

        // Submit partial blobs before the current block ends
        let submission_threshold = Duration::from_secs(SLOT_DURATION_SECS) - Duration::from_secs(1);
        let appending_duration = submission_threshold
            - (SystemTime::now().duration_since(UNIX_EPOCH).unwrap()
                - Duration::from_secs(self.ctx.current_block.timestamp));

        info!("Appending duration {:?}", appending_duration);

        let cancellation_token = self.cancellation_token.clone();
        let rpc_submit_clone = self.rpc_submit.clone();
        tokio::spawn(async move {
            tokio::time::sleep(appending_duration).await;
            cancellation_token.cancel();

            rpc_submit_clone.submit_partial_blobs(self.ctx).await;
            info!("Submitted partial blobs");
        });

        // Blob aggregation loop handling incoming blob segments for the current block
        let partial_blobs_sender = self.rpc_submit.get_partial_blobs_sender();
        loop {
            // Try to append the blob segment to the first partial blob with available space
            // If all are full, create a new partial blob
            if let Ok(segment) = self.input.recv().await {
                // Stop building when new block arrives
                if self.cancellation_token.is_cancelled() {
                    debug!("Previous Appending Blob Aggregator STOPPED");
                    break;
                }

                debug!("Received new blob segment: {:?}", segment);

                // TODO: Here we could allow to span blob segments over multiple blobs
                // Try to append the blob segment to the first partial blob with available space
                let mut appended = false;
                let mut new_partial_blobs = vec![];
                for partial_blob in partial_blobs.iter() {
                    if appended {
                        break;
                    }
                    if !partial_blob.can_append_segment(&segment.blob_segment_data) {
                        continue;
                    }

                    // Append new blob segment
                    debug!("Appending blob segment to {:?}", partial_blob);
                    let mut new_segments = partial_blob.segments().clone();
                    new_segments.push(segment.clone());

                    let mut new_data = partial_blob.data().clone().to_vec();
                    new_data.extend_from_slice(&segment.blob_segment_data.clone());
                    let new_data_bytes = Bytes::from(new_data);

                    new_partial_blobs.push(PartialBlob::new(new_segments, new_data_bytes));

                    appended = true
                }

                // If all partial blobs are full, create a new one
                if !appended {
                    debug!("Create new partial blob");
                    new_partial_blobs.push(PartialBlob::new(
                        vec![segment.clone()],
                        segment.blob_segment_data.clone(),
                    ));
                }

                // Override previous partial blobs, as we are only interested in the latest state
                partial_blobs = new_partial_blobs.clone();

                // Send the latest partial blobs state to the RPC submitter
                debug!("Send latest Appending Blob Aggregator state");
                partial_blobs_sender.send(new_partial_blobs).await;
            }
        }
    }
}
