use crate::primitives::blob_segment::BlobSegment;
use alloy_eips::eip4844::USABLE_BYTES_PER_BLOB;
use alloy_primitives::Bytes;

/// Intermediary blob structure for appending blob segments. Upon submission partial blobs are sealed
/// and submitted as a full blob.
///
/// NOTE: Make struct contents private to make PartialBlob immutable on the outside.
#[derive(Debug, Clone)]
pub struct PartialBlob {
    segments: Vec<BlobSegment>,
    data: Bytes,
}

impl PartialBlob {
    /// Creates a new `PartialBlob` instance
    ///
    /// # Arguments
    /// * `segments` - Blob segments to initialize new `PartialBlob` with
    /// * `data` - Blob data to initialize new `PartialBlob` with
    pub fn new(segments: Vec<BlobSegment>, data: Bytes) -> Self {
        Self { segments, data }
    }

    /// Check if the blob segment can be appended to the current blob
    pub fn can_append_segment(&self, segment_data: &Bytes) -> bool {
        self.data.len() + segment_data.len() < USABLE_BYTES_PER_BLOB
    }

    /// Return blob segments in the current partial blob
    pub fn segments(&self) -> &Vec<BlobSegment> {
        &self.segments
    }

    /// Return blob data in the current partial blob
    pub fn data(&self) -> &Bytes {
        &self.data
    }
}
