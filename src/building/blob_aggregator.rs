use crate::building::appending_blob_aggregator::AppendingBlobAggregator;
use crate::building::partial_blob::PartialBlob;
use crate::chain::new_heads_subscription::HeaderWithLogs;
use crate::primitives::blob_segment::{BlobSegment, BlobSegmentHash};
use crate::submission::rpc_submitter::RpcSubmitter;
use alloy_primitives::{BlockNumber, B256};
use alloy_rpc_types_eth::Header;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::field::debug;
use tracing::{debug, info};

/// Aggregation Context for aggregation tasks
#[derive(Debug)]
pub struct AggregationContext {
    pub current_block: Header,
}

/// Blob Aggregator struct, holding all necessary data for aggregation tasks
pub struct BlobAggregator {
    blob_segment_receiver: Receiver<BlobSegment>,
    new_heads_receiver: Receiver<HeaderWithLogs>,
    submitted_partial_blobs_receiver: Receiver<Vec<PartialBlob>>,

    rpc_submit: Arc<RpcSubmitter>,

    blob_segments_by_target_block_number: Arc<Mutex<HashMap<BlockNumber, Vec<BlobSegment>>>>,
    blob_segments_for_current_block: Arc<Mutex<HashMap<BlobSegmentHash, BlobSegment>>>,
}

impl BlobAggregator {
    /// Creates a new `BlobAggregator` instance
    ///
    /// # Arguments
    ///
    /// * `blob_segment_receiver` - New BlobSegments receiver from the RPC
    /// * `new_heads_receiver` - New block headers receiver from provider
    /// * `submitted_partial_blobs_receiver` - Submitted partial blobs receiver from RpcSubmitter
    /// * `rpc_submit` - RpcSubmitter for submitting blobs
    pub fn new(
        blob_segment_receiver: Receiver<BlobSegment>,
        new_heads_receiver: Receiver<HeaderWithLogs>,
        submitted_partial_blobs_receiver: Receiver<Vec<PartialBlob>>,
        rpc_submit: RpcSubmitter,
    ) -> Self {
        info!("Blob Aggregator INITIALIZED");

        Self {
            blob_segment_receiver,
            new_heads_receiver,
            submitted_partial_blobs_receiver,
            rpc_submit: Arc::new(rpc_submit),
            blob_segments_by_target_block_number: Arc::new(Mutex::new(HashMap::new())),
            blob_segments_for_current_block: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Accumulates BlobSegments received from the RPC, listens for new heads and manages aggregation tasks.
    ///
    /// When new head is received, the following tasks are executed:
    /// 1. Cancel previous aggregation task
    /// 2. Remove all PENDING / EXPIRED (target_block_number < current_block_number) / MINED BlobSegments
    /// 3. Add all accumulated BlobSegments for current block number
    /// 4. Start new aggregation task
    /// 5. Send all accumulated BlobSegments for current block number
    ///
    /// When the aggregated blob is submitted, the BlobSegments with None target block number are marked
    /// as PENDING to avoid being included on-chain again.
    pub async fn aggregate_blob_segments(mut self) {
        info!("Start aggregating blob segments");

        let input_channel_buffer_size = 10_000;
        let (input, _output) = broadcast::channel(input_channel_buffer_size);
        let mut current_block_number: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));

        // Accumulate BlobSegments
        let segments_by_block_number = self.blob_segments_by_target_block_number.clone();
        let segments_current = self.blob_segments_for_current_block.clone();
        let accumulator_input = input.clone();
        let current_block_number_guard = current_block_number.clone();
        tokio::spawn(async move {
            let mut blob_segment_receiver = &mut self.blob_segment_receiver;
            while let Some(blob_segment) = blob_segment_receiver.recv().await {
                debug!("Received new blob segment: {:?}", blob_segment);

                // TODO: Deduplicate same requests

                match blob_segment.block {
                    // Save blob segments with target block number
                    Some(block_number) => {
                        let mut blob_segments = segments_by_block_number.lock().await;

                        blob_segments
                            .entry(block_number)
                            .or_default()
                            .push(blob_segment.clone());

                        let current_block_number = current_block_number_guard.lock().await;
                        if block_number == *current_block_number {
                            accumulator_input.send(blob_segment).unwrap();
                        }
                    }
                    // Save blob segments with None target block number
                    None => {
                        let mut blob_segments = segments_current.lock().await;

                        blob_segments.insert(blob_segment.hash, blob_segment.clone());
                        accumulator_input.send(blob_segment).unwrap();
                    }
                }
            }
        });

        // Accumulate submitted blob segments
        let mut pending_blob_segment_to_expiration_block: Arc<Mutex<HashMap<B256, BlockNumber>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_inclusion_guard = pending_blob_segment_to_expiration_block.clone();
        let current_block_number_guard = current_block_number.clone();
        tokio::spawn(async move {
            let mut submitted_partial_blobs_receiver = &mut self.submitted_partial_blobs_receiver;
            while let Some(partial_blobs) = submitted_partial_blobs_receiver.recv().await {
                debug!("Received SUBMITTED partial blobs: {:?}", partial_blobs);

                // Save all pending blob segments with None target block number
                let current_block_number = current_block_number_guard.lock().await;
                let mut pending_inclusion = pending_inclusion_guard.lock().await;
                partial_blobs.iter().for_each(|blob| {
                    blob.segments().iter().for_each(|segment| {
                        if segment.block.is_none() {
                            pending_inclusion.insert(segment.hash, *current_block_number + 10);
                        }
                    })
                })
            }
        });

        // Listen for new heads and handle aggregation tasks
        let mut current_block_number_guard = current_block_number.clone();
        let mut cancellation_token: Option<CancellationToken> = None;
        let mut new_heads_receiver = &mut self.new_heads_receiver;
        while let Some((new_head, logs)) = new_heads_receiver.recv().await {
            debug!("Received new head: {:?}", new_head);

            *current_block_number_guard.lock().await = new_head.number;

            // Cancel previous aggregation task
            debug!("Cancelling previous aggregation task");
            if let Some(cancellation_token) = cancellation_token {
                cancellation_token.cancel();
            }
            let new_cancellation_token = CancellationToken::new();
            cancellation_token = Some(new_cancellation_token.clone());

            let pending_inclusion_guard = pending_blob_segment_to_expiration_block.clone();
            let mut pending_inclusion = pending_inclusion_guard.lock().await;
            debug!("Pending blob segments: {:?}", pending_inclusion);

            // Remove all blob segments with target block number
            let mut current_block_segments = self.blob_segments_for_current_block.lock().await;
            let mut count_before = current_block_segments.len();
            current_block_segments.retain(|_, b| {
                b.block.is_none() && pending_inclusion.get(&b.hash).unwrap_or(&0) < &new_head.number
            });
            debug!(
                "Removed all PENDING and TARGET BLOCK blob segments | Head: {} | Before: {} | After: {}",
                new_head.number,
                count_before,
                current_block_segments.len()
            );

            // Remove all mined blob segments with None target block number
            count_before = current_block_segments.len();
            logs.iter().for_each(|log| {
                current_block_segments.remove(&log.blobHash);
                pending_inclusion.remove(&log.blobHash);
            });
            debug!(
                "Removed all mined blob segments | Head: {}  | Before: {} | After: {}",
                new_head.number,
                count_before,
                current_block_segments.len()
            );

            // Add all accumulated blob segments for current block number
            let target_block_segments = self.blob_segments_by_target_block_number.lock().await;
            count_before = current_block_segments.len();
            if let Some(segments) = target_block_segments.get(&new_head.number) {
                current_block_segments.extend(segments.iter().cloned().map(|b| (b.hash, b)));
            }
            debug!(
                "Added all blob segments for current block | Head: {}  | Before: {} | After: {}",
                new_head.number,
                count_before,
                current_block_segments.len()
            );

            // Start new aggregation task
            debug!("Start new aggregation task");
            let blob_segment_receiver = input.subscribe();
            let rpc_submit = self.rpc_submit.clone();
            tokio::spawn(async move {
                debug!("Starting new Appending Blob Aggregator");
                let ctx = AggregationContext {
                    current_block: new_head,
                };
                let appending_blob_aggregator = AppendingBlobAggregator::new(
                    ctx,
                    blob_segment_receiver,
                    rpc_submit,
                    new_cancellation_token,
                );

                appending_blob_aggregator.build_blobs().await;
            });

            // Send all accumulated blob segments for current block number
            debug!("Send accumulated blob segments");
            current_block_segments.values().cloned().for_each(|bs| {
                input.send(bs).unwrap();
            });
        }
    }
}
