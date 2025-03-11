use crate::primitives::blob_segment::BlobSegment;
use crate::rpc::serialize::RawBlobSegment;
use jsonrpsee::server::middleware::http::Port;
use jsonrpsee::server::Server;
use jsonrpsee::tokio::sync::mpsc;
use jsonrpsee::tokio::task::JoinHandle;
use jsonrpsee::{tokio, RpcModule};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::{Duration, Instant};
use time::OffsetDateTime;
use tokio::sync::mpsc::error::SendTimeoutError;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// RPC Server for accepting new blob segments
#[derive(Debug)]
pub struct RpcServer {
    /// IPv4 address the RPC server will listen on
    ipv4addr: Ipv4Addr,
    /// Port number  the RPC server will listen on
    port: u16,
    /// Timeout duration for sending the input
    send_timeout: Duration,
}

impl RpcServer {
    /// Creates a new `RpcServer` instance
    ///
    /// # Arguments
    ///
    /// * `ipv4addr` - IPv4 address the RPC server will listen on
    /// * `port` - Port number the RPC server will listen on
    /// * `send_timeout` - Timeout duration for sending the input
    pub fn new(ipv4addr: Ipv4Addr, port: u16, send_timeout: Duration) -> Self {
        Self {
            ipv4addr,
            port,
            send_timeout,
        }
    }
}

/// Start RPC server, to start accepting blob segment requests
pub async fn start_rpc_server(
    rpc_server: RpcServer,
    input: mpsc::Sender<BlobSegment>,
    cancel: CancellationToken,
) -> eyre::Result<JoinHandle<()>> {
    // Create RPC server and bind it to the defined IP:PORT
    let addr = SocketAddrV4::new(rpc_server.ipv4addr, rpc_server.port);

    let server = Server::builder().http_only().build(addr).await?;

    // Define RPC endpoints
    let mut module = RpcModule::new(());

    module.register_async_method("eth_sendBlobSegment", move |params, _, _| {
        let input_clone = input.clone();
        handle_eth_send_blob_segment(input_clone, params, rpc_server.send_timeout)
    })?;

    // Start RPC server
    let handle = server.start(module);

    // Spawn an async task to monitor for RPC / CancellationToken events
    Ok(tokio::spawn(async move {
        info!("RPC server job: STARTED");
        tokio::select! {
            _ = cancel.cancelled() => {},
            _ = handle.stopped() => {
                info!("RPC server STOPPED");
                cancel.cancel();
            }
        }

        info!("RPC server job: STOPPED");
    }))
}

/// Parses a raw blob segment packet, remaps and forwards it to the input channel
async fn handle_eth_send_blob_segment(
    input: mpsc::Sender<BlobSegment>,
    params: jsonrpsee::types::Params<'static>,
    send_timeout: Duration,
) {
    debug!("Received new eth_sendBlobSegment request");

    // Parse RawBlobSegment
    let raw_blob_segment: RawBlobSegment = match params.one() {
        Ok(raw_blob_segment) => raw_blob_segment,
        Err(err) => {
            warn!(?err, "Failed to parse raw blob segment");
            return;
        }
    };

    // Remap RawBlobSegment <> BlobSegment
    let blob_segment = match raw_blob_segment.decode() {
        Ok(blob_segment_res) => blob_segment_res,
        Err(err) => {
            warn!(?err, "Failed to decode raw blob segment");
            return;
        }
    };

    // Send BlobSegment to the input channel
    send_blob_segment(input, blob_segment, send_timeout).await;
}

/// Send BlobSegment to the input channel
async fn send_blob_segment(
    input: mpsc::Sender<BlobSegment>,
    blob_segment: BlobSegment,
    send_timeout: Duration,
) {
    debug!("Sending NEW blob segment: {:?}", blob_segment);

    match input.send_timeout(blob_segment, send_timeout).await {
        Ok(()) => {}
        Err(SendTimeoutError::Timeout(_)) => {
            // Channel is most likely full, when the timeout elapsed
            warn!("Failed to send blob segment, timeout")
        }
        Err(SendTimeoutError::Closed(_)) => {}
    }
}
