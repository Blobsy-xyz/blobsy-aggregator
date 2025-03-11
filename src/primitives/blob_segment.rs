use alloy_primitives::{keccak256, Address, Bytes, B256};
use derivative::Derivative;
use integer_encoding::VarInt;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

pub type BlobSegmentHash = B256;

/// Structure to store BlobSegment data with additional fields for aggregation
#[derive(Derivative)]
#[derivative(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlobSegment {
    /// None means in the first possible block
    pub block: Option<u64>,
    pub min_timestamp: Option<u64>,
    pub max_timestamp: Option<u64>,
    /// Maximum fee that signer is willing to pay to include the blob segment.
    pub max_blob_segment_fee: u128,
    /// Blob segment data, to be aggregated
    pub blob_segment_data: Bytes,
    /// Contract on which the callbackPayload should be executed.
    pub callback_contract: Option<Address>,
    /// Payload to execute on callback contract.
    pub callback_payload: Option<Bytes>,
    /// Virtual hash generated from [blob_segment_data].
    pub hash: BlobSegmentHash,
    /// Unique id we generate.
    pub uuid: Uuid,

    /// Metadata about the BlobSegment, such as the received timestamp
    #[derivative(PartialEq = "ignore", Hash = "ignore")]
    pub metadata: Metadata,
}

impl BlobSegment {
    /// Recalculate blob segment hash and uuid.
    /// Hash is computed from blob segment bytes, callback contract, and callback payload
    pub fn hash_slow(&mut self) {
        self.hash = keccak256(&self.blob_segment_data);

        let uuid = {
            let mut buff = Vec::with_capacity(8 + 32);
            {
                let block = self.block.unwrap_or_default() as i64;
                buff.append(&mut block.encode_var_vec());
            }
            buff.extend_from_slice(self.hash.as_slice());

            let hash = {
                let mut res = [0u8; 16];
                let mut hasher = Sha256::new();
                hasher.update(res);
                hasher.update(&buff);

                let output = hasher.finalize();
                res.copy_from_slice(&output.as_slice()[0..16]);
                res
            };

            uuid::Builder::from_sha1_bytes(hash).into_uuid()
        };
        self.uuid = uuid;
    }
}

/// Metadata about the BlobSegment
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Metadata {
    pub received_at_timestamp: OffsetDateTime,
}

impl Default for Metadata {
    fn default() -> Self {
        Self {
            received_at_timestamp: OffsetDateTime::now_utc(),
        }
    }
}
