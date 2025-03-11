use crate::primitives::blob_segment::BlobSegment;
use alloy_primitives::{Address, Bytes, U128, U64};
use derivative::Derivative;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Struct to de/serialize json BlobSegment from RPC AP.
#[derive(Debug, Clone, Serialize, Deserialize, Derivative)]
#[derivative(PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RawBlobSegment {
    /// blockNumber (Optional) `String`, a hex encoded block number for which this blob segment is valid
    /// on. If nil or 0, blockNumber will default to the current pending block
    pub block_number: Option<U64>,
    /// Maximum fee that signer is willing to pay to include the blob segment
    pub max_blob_segment_fee: U128,
    /// Data to be included in blob, equivalent to the truncated BlobTx.Data field
    pub blob_segment_data: Bytes,
    /// callbackContract (Optional) on which the callbackPayload should be executed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_contract: Option<Address>,
    /// callbackPayload  (Optional) to execute on callback contract
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_payload: Option<Bytes>,
    /// Address of the blob segment sender
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signing_address: Option<Address>,
    /// minTimestamp (Optional) `Number`, the minimum timestamp for which this blob segment is valid, in
    /// seconds since the unix epoch
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_timestamp: Option<u64>,
    /// maxTimestamp (Optional) `Number`, the maximum timestamp for which this blob segment is valid, in
    /// seconds since the unix epoch
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_timestamp: Option<u64>,
    /// firstSeenAt `Number`, timestamp at which blob segment was first seen,
    /// used for ensuring we respect the order of uuid blob segments that
    /// were first received elsewhere
    #[derivative(PartialEq = "ignore")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_seen_at: Option<f64>,
}

impl RawBlobSegment {
    pub fn decode(self) -> Result<BlobSegment, ()> {
        // Create BlobSegment struct
        let block = self.block_number.unwrap_or_default().to();
        let mut blob_segment = BlobSegment {
            block: if block != 0 { Some(block) } else { None },
            blob_segment_data: self.blob_segment_data.clone(),
            max_blob_segment_fee: self.max_blob_segment_fee.to(),
            min_timestamp: self.min_timestamp,
            max_timestamp: self.max_timestamp,
            callback_contract: self.callback_contract,
            callback_payload: self.callback_payload,
            metadata: Default::default(),
            hash: Default::default(),
            uuid: Default::default(),
        };

        // Generate blob_segment hash and UUID
        blob_segment.hash_slow();

        Ok(blob_segment)
    }
}
