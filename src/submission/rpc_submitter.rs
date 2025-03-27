use crate::building::blob_aggregator::AggregationContext;
use crate::building::partial_blob::PartialBlob;
use alloy_eips::eip4844::builder::{SidecarBuilder, SimpleCoder};
use alloy_eips::eip4844::MAX_BLOBS_PER_BLOCK;
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::{Address, Bytes};
use alloy_provider::network::{
    AnyNetwork, EthereumWallet, NetworkWallet, TransactionBuilder, TransactionBuilder4844,
};
use alloy_provider::{Provider, ProviderBuilder, SendableTx};
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
        let signer_address: Address =
            <EthereumWallet as NetworkWallet<AnyNetwork>>::default_signer_address(
                &self.ethereum_wallet,
            );
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

        // Create BlobTransactionSidecar and BlobSegments data to be included in the transaction
        let mut sidecar: SidecarBuilder<SimpleCoder> =
            SidecarBuilder::with_capacity(partial_blobs.len());
        let mut blob_segments = vec![];
        partial_blobs.iter().for_each(|partial_blob| {
            let partial_blob_data = partial_blob.data().clone();

            let mut index: u64 = 0;
            partial_blob.segments().iter().for_each(|segment| {
                let length = segment.blob_segment_data.len() as u64;
                blob_segments.push(IBlobReceiver::BlobSegment {
                    receiverAddress: Default::default(),
                    firstBlobIndex: 0,
                    numBlobs: 0,
                    offset: index,
                    length: length - 1,
                    payload: Default::default(),
                    blobHash: segment.hash,
                });
                index += length;
            });

            //TODO: Hack to satisfy SimpleCoder
            let diff = 126976 - 31 - partial_blob_data.len();
            let empty_bytes = vec![0u8; diff];
            let mut partial_blob_data_vec = partial_blob_data.to_vec();
            partial_blob_data_vec.extend_from_slice(&empty_bytes);

            let full_blob_data = Bytes::from(partial_blob_data_vec);
            sidecar.ingest(&full_blob_data);
        });

        // TODO: Remove expect
        let sidecar = sidecar
            .build()
            .expect("Failed to create BlobTransactionSidecar");
        let data = BlobSplitter::postBlobCall::new((blob_segments,)).abi_encode();

        // Adjust execution priority fee, as this is the ordering criteria in GETH
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

    /// Get clone of the sender channel for submitted partial blobs.
    pub fn get_partial_blobs_sender(&self) -> mpsc::Sender<Vec<PartialBlob>> {
        self.partial_blobs_sender.clone()
    }
}
