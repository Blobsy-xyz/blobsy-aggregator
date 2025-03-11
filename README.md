# Blobsy Aggregator

Blobsy Aggregator is a service designed to aggregate blob segments (=non-zero part of a future full blob; data that will be sent by rollup), post them on the Ethereum blockchain, and register them in the `BlobSplitter` contract. It provides a new RPC endpoint (`eth_sendBlobSegment`) to accept new blob segments and attempts to aggregate them into blob carrying transaction on each new Ethereum block.

> **This service is actively being developed and is currently in its ALPHA stage. You can try it out on the Sepolia testnet using our RPC: https://rpc.blobsy.xyz/sepolia.**
> 
> `BlobSplitter` contract address on Sepolia: [0x757c85b754049E6338aFbA87C497F8286b255499](https://sepolia.etherscan.io/address/0x757c85b754049e6338afba87c497f8286b255499)

## 🎉 Features

- **RPC Endpoint**: Accepts new blob segments via the `eth_sendBlobSegment` endpoint.
- **Blob Aggregation**: Aggregates received blob segments into blob carrying transaction and posts them on-chain.
- **Blob Segment Registration**: Registers blob segments via the `BlobSplitter` contract, which emits events for each new blob segment to ensure transparency and easier identification.

## 🚀 Getting Started

### 📋 Prerequisites

- Rust (latest stable version)
- Cargo (Rust package manager)
- Ethereum node (for blockchain interaction)

### 🔧 Installation

1. Clone the repository:
    ```sh
    git clone https://github.com/Blobsy-xyz/blobsy-aggregator.git
    cd blobsy-aggregator
    ```

2. Build the project:
    ```sh
    cargo build --release
    ```

### ⚙️ Configuration

As the service is still under active development, configuration is done manually by setting the appropriate variable values in `/bin/run_blob_aggregator.rs`. Configuration via environment variables will be possible in the near future.

#### Required Configuration
```rust
let ws_provider = "".to_string();
let http_provider = "".to_string();
let private_key_signer: PrivateKeySigner = "".parse().expect("Failed to parse private key");
```

#### Optional Configuration
```rust
let server_ip = Ipv4Addr::new(0, 0, 0, 0);
let server_port: u16 = 9645;
let blob_splitter_address = address!("757c85b754049E6338aFbA87C497F8286b255499");
let input_channel_buffer_size = 10_000;
```

### 💻 Running the Service

Start the Blobsy Aggregator service:
```sh
cargo run --release --bin run_blob_aggregator
```

#### Other services
- Send blob segments to the service using the `eth_sendBlobSegment` RPC endpoint for testing:
```sh
cargo run --release --bin rpc_send_blob_segment
```

## 📖 Usage

### 📡 RPC Endpoint

Send blob segments to the service using the `eth_sendBlobSegment` RPC endpoint. The service will handle the aggregation and posting of blobs on-chain.

#### Expected API Payload

This structured payload allows users to specify key parameters for blob segment processing, including optional execution logic via callbacks and validity constraints based on timestamps.

| Field | Description | Required |
|-------|-------------|----------|
| `maxBlobSegmentFee` | The maximum fee, denominated in wei, that the sender is willing to pay for the inclusion of the blob segment. | Yes |
| `blobSegmentData` | The actual data to be included in the blob, provided as a hex-encoded string. | Yes |
| `blockNumber` | The block number at which the blob segment is intended to be included. This value is hex-encoded. | No |
| `callbackContract` | The Ethereum address of a smart contract that will receive a callback once the blob segment is processed. | No |
| `callbackPayload` | Additional data, hex-encoded, that will be passed to the callback contract during execution. | No |
| `signingAddress` | The Ethereum address of the sender, used to authenticate and verify the origin of the blob segment. | No |
| `minTimestamp` | The earliest Unix timestamp (in seconds) from which this blob segment is considered valid. | No |
| `maxTimestamp` | The latest Unix timestamp (in seconds) until which this blob segment remains valid. | No |

#### Example Payload

```json
{
  "blockNumber": "0x1a",
  "maxBlobSegmentFee": "1000000000000000000",
  "blobSegmentData": "0x68656c6c6f",
  "callbackContract": "0x1234567890abcdef1234567890abcdef12345678",
  "callbackPayload": "0xabcdef",
  "signingAddress": "0xabcdefabcdefabcdefabcdefabcdefabcdef",
  "minTimestamp": 1633024800,
  "maxTimestamp": 1633111200
}
```

#### cURL Example

```sh
curl -X POST https://rpc.blobsy.xyz/sepolia \
-H "Content-Type: application/json" \
-d '{
  "jsonrpc": "2.0",
  "method": "eth_sendBlobSegment",
  "params": [{
    "maxBlobSegmentFee": "1000000000000000000",
    "blobSegmentData": "0x68656c6c6f"
  }],
  "id": 1
}'
```

## 🤝 Contributing

Contributions are welcome! Please review the project's documentation and follow the contribution guidelines.

## ❤️ Special Thanks

Special thanks to [Lin Oshitani](https://github.com/linoscope/) for creating the pseudocode for the `BlobSplitter` contract: https://hackmd.io/@linoscope/blob-sharing-for-based-rollups.

## License

This project is licensed under the MIT License.

For more information, please review the entire project, as it is well documented.