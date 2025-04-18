use crate::primitives::blob_segment::BlobSegment;
use crate::submission::optimized_blob_coder::OptimizedBlobCoder;
use alloy_eips::eip4844::USABLE_BYTES_PER_BLOB;
use alloy_primitives::Bytes;
use std::cmp::Ordering;

/// Intermediary blob structure for appending blob segments. Upon submission partial blobs are sealed
/// and submitted as a full blob.
///
/// NOTE: Make struct contents private to make PartialBlob immutable on the outside.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartialBlob {
    segments: Vec<BlobSegment>,
    data: Bytes,
    total_fee: u128,
}

impl PartialBlob {
    /// Creates a new `PartialBlob` instance
    ///
    /// # Arguments
    /// * `segments` - Blob segments to initialize new `PartialBlob` with
    /// * `data` - Blob data to initialize new `PartialBlob` with
    /// * `total_fee` - Total fee the blob segments are willing to pay
    pub fn new(segments: Vec<BlobSegment>, data: Bytes, total_fee: u128) -> Self {
        Self {
            segments,
            data,
            total_fee,
        }
    }

    /// Check if the blob segment can be appended to the current blob
    pub fn can_append_segment(&self, segment_data: &Bytes) -> bool {
        let mut segment_len = segment_data.len();
        segment_len += OptimizedBlobCoder::LENGTH_PREFIX_SIZE_BYTES;

        self.data.len() + segment_len < USABLE_BYTES_PER_BLOB
    }

    /// Return blob segments in the current partial blob
    pub fn segments(&self) -> &Vec<BlobSegment> {
        &self.segments
    }

    /// Return blob data in the current partial blob
    pub fn data(&self) -> &Bytes {
        &self.data
    }

    /// Return total fee the blob segments are willing to pay
    pub fn total_fee(&self) -> u128 {
        self.total_fee
    }
}

// Implement custom DESCENDING ordering by total_fee for PartialBlob
impl Ord for PartialBlob {
    fn cmp(&self, other: &Self) -> Ordering {
        other.total_fee.cmp(&self.total_fee)
    }

    fn max(self, other: Self) -> Self
    where
        Self: Sized,
    {
        if self.total_fee >= other.total_fee {
            self
        } else {
            other
        }
    }

    fn min(self, other: Self) -> Self
    where
        Self: Sized,
    {
        if self.total_fee <= other.total_fee {
            self
        } else {
            other
        }
    }

    fn clamp(self, min: Self, max: Self) -> Self
    where
        Self: Sized,
        Self: PartialOrd,
    {
        if self.total_fee < min.total_fee {
            min
        } else if self.total_fee > max.total_fee {
            max
        } else {
            self
        }
    }
}

impl PartialOrd for PartialBlob {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }

    fn lt(&self, other: &Self) -> bool {
        self.total_fee < other.total_fee
    }

    fn le(&self, other: &Self) -> bool {
        self.total_fee <= other.total_fee
    }

    fn gt(&self, other: &Self) -> bool {
        self.total_fee > other.total_fee
    }

    fn ge(&self, other: &Self) -> bool {
        self.total_fee >= other.total_fee
    }
}
