use alloy_primitives::address;
use alloy_rpc_types_eth::pubsub::SubscriptionKind::NewHeads;
use alloy_signer_local::PrivateKeySigner;
use blobsy_aggregator::building::blob_aggregator::BlobAggregator;
use blobsy_aggregator::chain::new_heads_subscription::NewHeadsSubscription;
use blobsy_aggregator::rpc::rpc_server::{start_rpc_server, RpcServer};
use blobsy_aggregator::submission::rpc_submitter::RpcSubmitter;
use std::net::Ipv4Addr;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::info;

#[tokio::main]
async fn main() {
    // Initialize tracing subscriber
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    info!("Blob Aggregator STARTED");

    // Configuration
    let ws_provider = "".to_string();
    let http_provider = "".to_string();
    let private_key_signer: PrivateKeySigner = "".parse().expect("Failed to parse private key");

    let server_ip = Ipv4Addr::new(0, 0, 0, 0);
    let server_port: u16 = 9645;
    let blob_splitter_address = address!("757c85b754049E6338aFbA87C497F8286b255499");
    let input_channel_buffer_size = 10_000;

    let global_cancellation = CancellationToken::new();

    // Start RPC server
    info!("Start RPC server");
    let (blob_segment_sender, blob_segment_receiver) = mpsc::channel(input_channel_buffer_size);
    let rpc = RpcServer::new(server_ip, server_port, Duration::from_millis(500));
    start_rpc_server(rpc, blob_segment_sender, global_cancellation.clone())
        .await
        .expect("Failed to start RPC server");

    // Subscribe to new heads
    info!("Subscribe to new heads");
    let (new_heads_sender, new_heads_receiver) = mpsc::channel(input_channel_buffer_size);
    let new_heads = NewHeadsSubscription::new(ws_provider);
    new_heads
        .subscribe_to_new_heads(
            new_heads_sender,
            blob_splitter_address,
            global_cancellation.clone(),
        )
        .await
        .expect("Failed to subscribe to new heads");

    // Build aggregated blobs
    let (submitted_partial_blobs_sender, submitted_partial_blobs_receiver) =
        mpsc::channel(input_channel_buffer_size);

    info!("Initialize RPC Submitter");
    let rpc_submit = RpcSubmitter::new(
        http_provider,
        private_key_signer,
        blob_splitter_address,
        global_cancellation.clone(),
        submitted_partial_blobs_sender.clone(),
    )
    .await;

    info!("Initialize Blob Aggregator");
    let aggregator = BlobAggregator::new(
        blob_segment_receiver,
        new_heads_receiver,
        submitted_partial_blobs_receiver,
        rpc_submit,
    );
    aggregator.aggregate_blob_segments().await;

    info!("Blob Aggregator STOPPED");
}
