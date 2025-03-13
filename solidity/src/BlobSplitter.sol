// SPDX-License-Identifier: MIT
pragma solidity ^0.8.10;

/**
 * @notice Interface for contracts that can act as receivers in the
 *         blob-sharing protocol. These contracts are expected to
 *         implement the `receiveBlob` function to process incoming
 *         blob segments.
 */
interface IBlobReceiver {
    /**
     * @notice Represents a segment of a blob in the blob-sharing protocol.
     *         Each blob is split into segments, and each segment is
     *         intended to be sent to a specified receiver contract for
     *         processing.
     */
    struct BlobSegment {
        // Address of the receiver contract for this blob segment
        address receiverAddress;
        // Unique identifier for the blob segment
        bytes32 blobHash;
        // Index of the first blob
        uint64 firstBlobIndex;
        // Number of blobs that this segment spans.
        uint8 numBlobs;
        // Offset within the blob where this segment starts
        uint64 offset;
        // Length of the segment in bytes
        uint64 length;
        // Additional payload to pass to the receiver contract
        bytes payload;
    }

    /**
     * @notice Processes a blob segment sent via the blob-sharing protocol.
     * @param segment Struct containing details of the blob segment
     */
    function receiveBlob(BlobSegment calldata segment) external;
}

contract BlobSplitter {
    event BlobSegmentPosted(
        address indexed receiverAddress,
        bytes32 indexed blobHash,
        uint64 firstBlobIndex,
        uint8 numBlobs,
        uint64 offset,
        uint64 length,
        bytes payload,
        bool success
    );

    /**
     * @notice Posts an array of blob segments to their designated receiver
     *         contracts as part of the blob-sharing protocol.
     * @param segments Array of blob segments to be dispatched.
     */
    function postBlob(IBlobReceiver.BlobSegment[] calldata segments) external {
        for (uint256 i = 0; i < segments.length; ++i) {
            IBlobReceiver.BlobSegment calldata segment = segments[i];
            IBlobReceiver receiver = IBlobReceiver(segment.receiverAddress);

            // Dispatch the blob segment to the receiver contract
            bool success = false;
            if (segment.receiverAddress != address(0)) {
                (success, ) = address(receiver).call(abi.encodeWithSelector(receiver.receiveBlob.selector, segment));
            }

            // Emit an event to indicate that the blob segment has been dispatched
            emit BlobSegmentPosted(
                segment.receiverAddress,
                segment.blobHash,
                segment.firstBlobIndex,
                segment.numBlobs,
                segment.offset,
                segment.length,
                segment.payload,
                success
            );
        }
    }
}
