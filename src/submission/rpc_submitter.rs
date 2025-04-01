use crate::building::blob_aggregator::AggregationContext;
use crate::building::partial_blob::PartialBlob;
use crate::submission::optimized_blob_coder::OptimizedBlobCoder;
use crate::submission::rpc_submitter::IBlobReceiver::BlobSegment;
use alloy_eips::eip4844::builder::SidecarBuilder;
use alloy_eips::eip4844::{BlobTransactionSidecar, MAX_BLOBS_PER_BLOCK, USABLE_BYTES_PER_BLOB};
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::{Address, Bytes};
use alloy_provider::network::{EthereumWallet, TransactionBuilder, TransactionBuilder4844};
use alloy_provider::{Provider, ProviderBuilder, SendableTx, WalletProvider};
use alloy_rpc_types_eth::{FeeHistory, TransactionRequest};
use alloy_signer_local::PrivateKeySigner;
use alloy_sol_types::{sol, SolCall};
use jsonrpsee::client_transport::ws::Url;
use std::sync::Arc;
use tokio::sync::mpsc::Receiver;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

// Codegen from ABI file to interact with the contract.
sol!(BlobSplitter, "src/abi/BlobSplitter.json");

/// RPC Submitter for sealing and submitting partial blobs.
pub struct RpcSubmitter {
    http_provider: String,
    ethereum_wallet: EthereumWallet,
    blob_splitter_address: Address,
    cancellation_token: CancellationToken,

    partial_blobs: Arc<Mutex<Vec<PartialBlob>>>,
    partial_blobs_sender: mpsc::Sender<Vec<PartialBlob>>,
    submitted_partial_blobs_sender: mpsc::Sender<Vec<PartialBlob>>,
}

impl RpcSubmitter {
    /// Creates a new `RpcSubmitter` instance and starts listening for new submission requests.
    ///
    /// # Arguments
    /// * `http_provider` - HTTP provider URL to submit blob transactions to
    /// * `signer` - Private key signer to sign transactions
    /// * `blob_splitter_address` - BlobSplitter contract address
    /// * `cancellation_token` - Cancellation token for stopping the submission
    /// * `submitted_partial_blobs_sender` - Sender channel for submitted partial blobs
    ///
    /// NOTE: As we don't want to split initialization and listening for new submission requests into
    /// separate methods, this constructor is async.
    pub async fn new(
        http_provider: String,
        signer: PrivateKeySigner,
        blob_splitter_address: Address,
        cancellation_token: CancellationToken,
        submitted_partial_blobs_sender: mpsc::Sender<Vec<PartialBlob>>,
    ) -> Self {
        let ethereum_wallet = EthereumWallet::new(signer);
        let input_channel_buffer_size = 10_000;
        let (partial_blobs_sender, partial_blobs_receiver) =
            mpsc::channel(input_channel_buffer_size);

        let ret = Self {
            http_provider,
            ethereum_wallet,
            blob_splitter_address,
            cancellation_token,
            partial_blobs: Arc::new(Mutex::new(vec![])),
            partial_blobs_sender,
            submitted_partial_blobs_sender,
        };

        // Listen for new submission requests
        ret.listen_for_new_submission_requests(partial_blobs_receiver)
            .await;

        info!("RPC Submitter INITIALIZED");
        ret
    }

    /// Listen for new submission requests of partial blobs and store latest state.
    async fn listen_for_new_submission_requests(
        &self,
        partial_blobs_receiver: Receiver<Vec<PartialBlob>>,
    ) {
        info!("Listen for new submission requests");

        let cancellation_token = self.cancellation_token.clone();
        let partial_blobs_mutex = self.partial_blobs.clone();

        tokio::spawn(async move {
            let mut partial_blobs_receiver = partial_blobs_receiver;
            loop {
                // Listen for new partial blobs and store latest state
                if let Some(partial_blobs) = partial_blobs_receiver.recv().await {
                    if cancellation_token.is_cancelled() {
                        break;
                    }

                    debug!("Received new partial blobs: {:?}", partial_blobs);

                    let mut partial_blobs_lock = partial_blobs_mutex.lock().await;
                    *partial_blobs_lock = partial_blobs;
                }
            }
        });
    }

    /// Trigger submission of partial blobs with given context.
    pub async fn submit_partial_blobs(&self, ctx: AggregationContext) {
        info!("Submitting partial blobs");

        let mut partial_blobs = self.partial_blobs.lock().await;
        if partial_blobs.is_empty() {
            error!("Something wrong, no partial blobs to submit");
            return;
        }

        // TODO: Figure out how to define provider only once
        let provider = ProviderBuilder::new()
            .wallet(&self.ethereum_wallet)
            .on_http(Url::parse(self.http_provider.as_str()).unwrap());

        // Fetch fee history, to manually adjust the transaction fees
        let fee_history = provider
            .get_fee_history(1, BlockNumberOrTag::Latest, &[75f64])
            .await
            .unwrap();

        // Create Blob TxRequest to be submitted
        let signer_address: Address = provider.default_signer_address();
        let tx_request = self
            .create_tx(partial_blobs.clone(), fee_history)
            .await
            .with_from(signer_address);

        debug!("Submission transaction request: {:?}", tx_request);

        // Automatically fill the missing fields in the transaction request
        let tx_filled = match provider.fill(tx_request).await.unwrap() {
            SendableTx::Builder(_) => None,
            SendableTx::Envelope(tx_envelope) => Some(tx_envelope),
        };

        // Send the transaction
        let pending_transaction = provider.send_tx_envelope(tx_filled.unwrap()).await.unwrap();

        info!(
            "Submission transaction SENT [target_block: {}]: {}",
            ctx.current_block.number,
            pending_transaction.tx_hash()
        );

        // Clear partial blobs
        partial_blobs.clear();
    }

    /// Construct TransactionRequest to transform partial blobs into a blob carrying transaction
    /// being sent to the BlobSplitter contract.
    async fn create_tx(
        &self,
        partial_blobs: Vec<PartialBlob>,
        fee_history: FeeHistory,
    ) -> TransactionRequest {
        let mut partial_blobs = partial_blobs.clone();
        // TODO: Sort to submit only the best partial blobs
        // partial_blobs.sort();

        if partial_blobs.len() > MAX_BLOBS_PER_BLOCK {
            partial_blobs.truncate(MAX_BLOBS_PER_BLOCK);
        }

        // Immediately signal partial blobs are being submitted
        self.submitted_partial_blobs_sender
            .send(partial_blobs.clone())
            .await;

        // Create sidecar and Solidity structs
        let (sidecar, blob_segments) =
            RpcSubmitter::create_sidecar_and_solidity_structs(partial_blobs);
        let data = BlobSplitter::postBlobCall::new((blob_segments,)).abi_encode();

        // Adjust execution priority fee, as this is the ordering criteria of blob transactions
        let rewards = fee_history.reward.unwrap();
        let priority_fee = rewards.get(0).unwrap().get(0).unwrap().clone();
        let max_fee = priority_fee + fee_history.base_fee_per_gas.last().unwrap();

        // Create and return the transaction request
        TransactionRequest::default()
            .with_to(self.blob_splitter_address)
            .with_blob_sidecar(sidecar)
            .with_max_fee_per_gas(max_fee)
            .with_max_priority_fee_per_gas(priority_fee)
            .with_input(data)
    }

    fn create_sidecar_and_solidity_structs(
        partial_blobs: Vec<PartialBlob>,
    ) -> (BlobTransactionSidecar, Vec<BlobSegment>) {
        // Create BlobTransactionSidecar and BlobSegments data to be included in the transaction
        let coder = OptimizedBlobCoder::new(true);
        let mut sidecar = SidecarBuilder::from_coder_and_capacity(coder, partial_blobs.len());
        let mut blob_segments = vec![];
        partial_blobs
            .iter()
            .enumerate()
            .for_each(|(i, partial_blob)| {
                let partial_blob_data = partial_blob.data().clone();

                // Create BlobSegment structs for Solidity contract call
                let mut offset: u64 = 0;
                partial_blob.segments().iter().for_each(|segment| {
                    let length = segment.blob_segment_data.len() as u64;
                    blob_segments.push(BlobSegment {
                        receiverAddress: segment.callback_contract.unwrap_or_default(),
                        firstBlobIndex: i as u64,
                        numBlobs: 1,
                        offset,
                        length,
                        payload: segment.callback_payload.clone().unwrap_or_default(),
                        blobHash: segment.hash,
                    });
                    offset += length;
                });

                // Pad remaining blob space with zeros, so we fill the whole blob
                let mut diff = USABLE_BYTES_PER_BLOB as i64;
                diff -= partial_blob_data.len() as i64; // Subtract blob data length

                // Subtract header size
                if i == 0 {
                    diff -= OptimizedBlobCoder::HEADER_SIZE_BYTES as i64;
                }

                // Subtract blob length size
                if coder.should_prepend_length() {
                    diff -= OptimizedBlobCoder::LENGTH_PREFIX_SIZE_BYTES as i64;
                }

                // Handle negative diff, which means more than one blob is needed
                if diff < 0 {
                    diff += USABLE_BYTES_PER_BLOB as i64;
                }

                let empty_bytes = vec![0u8; diff as usize];
                let mut partial_blob_data_vec = partial_blob_data.to_vec();
                partial_blob_data_vec.extend_from_slice(&empty_bytes);

                // Create & ingest full blob data
                let full_blob_data = Bytes::from(partial_blob_data_vec);
                sidecar.ingest(&full_blob_data);
            });

        // TODO: Remove expect
        let sidecar = sidecar
            .build()
            .expect("Failed to create BlobTransactionSidecar");

        (sidecar, blob_segments)
    }

    /// Get clone of the sender channel for submitted partial blobs.
    pub fn get_partial_blobs_sender(&self) -> mpsc::Sender<Vec<PartialBlob>> {
        self.partial_blobs_sender.clone()
    }
}

#[cfg(test)]
mod tests {
    use crate::building::partial_blob::PartialBlob;
    use crate::primitives::blob_segment::BlobSegment;
    use crate::submission::optimized_blob_coder::OptimizedBlobCoder;
    use crate::submission::rpc_submitter::RpcSubmitter;
    use alloy_eips::eip4844::USABLE_BYTES_PER_BLOB;
    use alloy_primitives::{Bytes, FixedBytes, U256};

    #[test]
    fn test_fully_pack_one_blob() {
        let mut length = USABLE_BYTES_PER_BLOB;
        length -= OptimizedBlobCoder::HEADER_SIZE_BYTES;
        length -= OptimizedBlobCoder::LENGTH_PREFIX_SIZE_BYTES;

        // Create a blob with data
        let partial_blob_data = Bytes::from(vec![255u8; length]);
        let partial_blob = create_partial_blob(partial_blob_data);

        // Create sidecar and solidity structs
        let (sidecar, blob_segments) =
            RpcSubmitter::create_sidecar_and_solidity_structs(vec![partial_blob]);

        // Assert there is only one blob with 1 segment
        assert_eq!(sidecar.blobs.len(), 1);
        assert_eq!(blob_segments.len(), 1);

        // Assert all field elements are greater than zero
        let field_elements_as_u256 = sidecar.blobs[0]
            .chunks(32) // U256 is 32 bytes (256 bits)
            .map(|chunk| FixedBytes::from_slice(chunk).into())
            .collect::<Vec<U256>>();
        assert!(field_elements_as_u256.iter().all(|fe| *fe > U256::ZERO));
    }

    #[test]
    fn test_fully_pack_two_blobs() {
        let mut length = USABLE_BYTES_PER_BLOB;
        length -= OptimizedBlobCoder::HEADER_SIZE_BYTES;
        length -= OptimizedBlobCoder::LENGTH_PREFIX_SIZE_BYTES;

        // Create a blob with data
        let partial_blob_data_1 = Bytes::from(vec![255u8; length]);
        let partial_blob_1 = create_partial_blob(partial_blob_data_1);

        length += OptimizedBlobCoder::HEADER_SIZE_BYTES; // Header is present only in the first blob
        let partial_blob_data_2 = Bytes::from(vec![255u8; length]);
        let partial_blob_2 = create_partial_blob(partial_blob_data_2);

        // Create sidecar and solidity structs
        let (sidecar, blob_segments) =
            RpcSubmitter::create_sidecar_and_solidity_structs(vec![partial_blob_1, partial_blob_2]);

        // Assert there are two blobs with 1 segment each
        assert_eq!(sidecar.blobs.len(), 2);
        assert_eq!(blob_segments.len(), 2);

        // Assert ALL field elements of BLOB 1 are greater than zero
        let field_elements_as_u256 = sidecar.blobs[0]
            .chunks(32) // U256 is 32 bytes (256 bits)
            .map(|chunk| FixedBytes::from_slice(chunk).into())
            .collect::<Vec<U256>>();
        assert!(field_elements_as_u256.iter().all(|fe| *fe > U256::ZERO));

        // Assert ALL field elements of BLOB 2 are greater than zero
        let field_elements_as_u256 = sidecar.blobs[1]
            .chunks(32) // U256 is 32 bytes (256 bits)
            .map(|chunk| FixedBytes::from_slice(chunk).into())
            .collect::<Vec<U256>>();
        assert!(field_elements_as_u256.iter().all(|fe| *fe > U256::ZERO));
    }

    #[test]
    fn test_pad_first_blob() {
        let mut length = 31;
        length -= OptimizedBlobCoder::LENGTH_PREFIX_SIZE_BYTES;

        // Create a blob with data
        let partial_blob_data_1 = Bytes::from(vec![255u8; length]);
        let partial_blob_1 = create_partial_blob(partial_blob_data_1);

        length = USABLE_BYTES_PER_BLOB;
        length -= OptimizedBlobCoder::LENGTH_PREFIX_SIZE_BYTES;
        let partial_blob_data_2 = Bytes::from(vec![255u8; length]);
        let partial_blob_2 = create_partial_blob(partial_blob_data_2);

        // Create sidecar and solidity structs
        let (sidecar, blob_segments) =
            RpcSubmitter::create_sidecar_and_solidity_structs(vec![partial_blob_1, partial_blob_2]);

        // Assert there are two blobs with 1 segment each
        assert_eq!(sidecar.blobs.len(), 2);
        assert_eq!(blob_segments.len(), 2);

        // Assert ONLY FIRST TWO field element of BLOB 2 is greater than zero and the rest are zero
        let field_elements_as_u256 = sidecar.blobs[0]
            .chunks(32) // U256 is 32 bytes (256 bits)
            .map(|chunk| FixedBytes::from_slice(chunk).into())
            .collect::<Vec<U256>>();
        assert!(field_elements_as_u256[0] > U256::ZERO);
        assert!(field_elements_as_u256[1] > U256::ZERO);
        assert!(field_elements_as_u256[2..]
            .iter()
            .all(|fe| *fe == U256::ZERO));

        // Assert ALL field elements of BLOB 2 are greater than zero
        let field_elements_as_u256 = sidecar.blobs[1]
            .chunks(32) // U256 is 32 bytes (256 bits)
            .map(|chunk| FixedBytes::from_slice(chunk).into())
            .collect::<Vec<U256>>();
        assert!(field_elements_as_u256.iter().all(|fe| *fe > U256::ZERO));
    }

    #[test]
    fn test_overflow_into_two_blobs() {
        let mut length = USABLE_BYTES_PER_BLOB;
        length -= OptimizedBlobCoder::HEADER_SIZE_BYTES;
        length -= OptimizedBlobCoder::LENGTH_PREFIX_SIZE_BYTES;
        length += 1; // Add 1 byte to overflow into the second blob

        // Create a blob with data
        let partial_blob_data = Bytes::from(vec![254u8; length]);
        let partial_blob = PartialBlob::new(
            vec![
                BlobSegment {
                    block: None,
                    min_timestamp: None,
                    max_timestamp: None,
                    max_blob_segment_fee: 0,
                    blob_segment_data: partial_blob_data.clone().slice(0..(length - 1)),
                    callback_contract: None,
                    callback_payload: None,
                    hash: Default::default(),
                    uuid: Default::default(),
                    metadata: Default::default(),
                },
                BlobSegment {
                    block: None,
                    min_timestamp: None,
                    max_timestamp: None,
                    max_blob_segment_fee: 0,
                    blob_segment_data: partial_blob_data.clone().slice(length - 1..length),
                    callback_contract: None,
                    callback_payload: None,
                    hash: Default::default(),
                    uuid: Default::default(),
                    metadata: Default::default(),
                },
            ],
            partial_blob_data,
        );

        // Create sidecar and solidity structs
        let (sidecar, blob_segments) =
            RpcSubmitter::create_sidecar_and_solidity_structs(vec![partial_blob]);

        // Assert there are two blobs with two segments
        assert_eq!(sidecar.blobs.len(), 2);
        assert_eq!(blob_segments.len(), 2);

        // Assert ALL field elements of BLOB 1 are greater than zero
        let field_elements_as_u256 = sidecar.blobs[0]
            .chunks(32) // U256 is 32 bytes (256 bits)
            .map(|chunk| FixedBytes::from_slice(chunk).into())
            .collect::<Vec<U256>>();
        assert!(field_elements_as_u256.iter().all(|fe| *fe > U256::ZERO));

        // Assert ONLY FIRST field element of BLOB 2 is greater than zero and the rest are zero
        let field_elements_as_u256 = sidecar.blobs[1]
            .chunks(32) // U256 is 32 bytes (256 bits)
            .map(|chunk| FixedBytes::from_slice(chunk).into())
            .collect::<Vec<U256>>();

        assert!(field_elements_as_u256[0] > U256::ZERO);
        assert!(field_elements_as_u256[1..]
            .iter()
            .all(|fe| *fe == U256::ZERO));
    }

    fn create_partial_blob(data: Bytes) -> PartialBlob {
        PartialBlob::new(
            vec![BlobSegment {
                block: None,
                min_timestamp: None,
                max_timestamp: None,
                max_blob_segment_fee: 0,
                blob_segment_data: data.clone(),
                callback_contract: None,
                callback_payload: None,
                hash: Default::default(),
                uuid: Default::default(),
                metadata: Default::default(),
            }],
            data,
        )
    }
}
