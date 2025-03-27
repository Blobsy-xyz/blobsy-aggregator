use alloy_eips::eip4844::builder::{PartialSidecar, SidecarCoder};
use alloy_eips::eip4844::utils::WholeFe;
use alloy_eips::eip4844::{Blob, FIELD_ELEMENT_BYTES_USIZE, USABLE_BITS_PER_FIELD_ELEMENT};
use alloy_primitives::hex::ToHexExt;

/// Optimized BLOB coder that uses up to 254 bits (31.75 bytes) of each field element.
///
/// This coder maximizes storage efficiency by using:
/// - The lower 254 bits of each field element (31.75 bytes)
/// - The top 2 bits of each field element must remain 0 to preserve EIP-4844 KZG commitment requirements
///
///         The BLS12-381 scalar field (used in KZG commitments) has a modulus close to 2^255.
///         However, it is not exactly 2^255, so the top 2 bits of the field element must be 0.
///         This ensures the field element remains valid.
///
/// Encoding process:
/// - Before encoding data, the HEADER (blob coder identifier) is encoded into the first field element of the first blob
/// - Data is OPTIONALLY pre-pended with a 32-bit (4 bytes) big-endian length prefix
/// - Data is converted to a bitstream and packed into consecutive field elements
/// - Each field element stores up to 254 bits of the stream
/// - The last field element is padded with zeros if needed
#[derive(Clone, Copy, Debug, Default)]
pub struct OptimizedBlobCoder {
    prepend_length: bool,

    header_prepended: bool,
}

impl OptimizedBlobCoder {
    const UNALLOCATED_BITS_PER_FE: usize = 256 - USABLE_BITS_PER_FIELD_ELEMENT;
    const LENGTH_PREFIX_BITS: usize = 32;

    /// Create a new instance of `OptimizedCoder`.
    pub fn new(prepend_length: bool) -> Self {
        Self {
            prepend_length,
            header_prepended: false,
        }
    }

    /// Get the name of the coder.
    const fn get_name(&self) -> &'static str {
        const NAME: &str = "OptimizedCoder(pl=true)";
        const _: () = assert!(
            NAME.len() < FIELD_ELEMENT_BYTES_USIZE,
            "Coder name must fit into 31 bytes (so it's encoded as one FE)"
        );
        NAME
    }

    /// Decode a single piece of data from an iterator of valid field elements.
    ///
    /// Returns `Ok(Some(data))` if there is data.
    /// Returns `Ok(None)` if there is no data (empty iterator, length prefix is 0).
    /// Returns `Err(())` if there is an error.
    fn decode_one<'a, I>(&self, iter: &mut I) -> Result<Option<Vec<u8>>, ()>
    where
        I: Iterator<Item = WholeFe<'a>> + Clone,
    {
        let Some(first) = iter.next() else {
            return Ok(None);
        };

        let mut fes = Vec::new();
        fes.push(first.as_ref().try_into().unwrap());

        // Convert field elements to bits
        let bits = Self::field_elements_to_bits(&fes);

        // Set up initial values
        const MAX_ALLOCATION_SIZE: usize = 2_097_152; // 2 MiB
        let mut num_bytes = 0;
        let mut start_bit = 0;

        // Read data length
        if self.prepend_length {
            // Extract length prefix (32 bits)
            let length_bits = &bits[0..Self::LENGTH_PREFIX_BITS];
            let length_bytes = Self::bits_to_bytes(length_bits);
            num_bytes = u32::from_be_bytes(length_bytes.try_into().unwrap()) as usize;
            start_bit = Self::LENGTH_PREFIX_BITS;
        } else {
            // If no length prefix, we assume the data is the entire blob
            // Increase fe_count by +1 to account already consumed first FE
            let fe_count = iter.clone().count() + 1;
            num_bytes = fe_count * FIELD_ELEMENT_BYTES_USIZE;

            // Subtract unallocated bits from the total number of bytes
            let unallocated_bits_count = fe_count * Self::UNALLOCATED_BITS_PER_FE;
            num_bytes -= unallocated_bits_count.div_ceil(8);
        }

        // If length is 0, we're done
        if num_bytes == 0 {
            return Ok(None);
        }

        // Limit allocation size for safety
        if num_bytes > MAX_ALLOCATION_SIZE {
            return Err(());
        }

        // Calculate how many more bits we need
        let bits_needed = num_bytes * 8;
        let mut bits_already_have = bits.len();

        // Subtract length prefix bits (if any)
        if self.prepend_length {
            bits_already_have = bits_already_have - Self::LENGTH_PREFIX_BITS;
        }

        // Calculate how many more field elements we need
        let additional_bits_needed = bits_needed.saturating_sub(bits_already_have);
        let additional_fes_needed = additional_bits_needed.div_ceil(USABLE_BITS_PER_FIELD_ELEMENT);

        // Collect the remaining field elements
        for _ in 0..additional_fes_needed {
            let fe = iter.next();
            if !fe.is_some() {
                // If we run out of field elements, stop processing
                break;
            }
            fes.push(fe.unwrap().as_ref().try_into().unwrap());
        }

        // Re-extract all bits now that we have enough field elements
        let all_bits = Self::field_elements_to_bits(&fes);

        // Take data bits (after the length prefix) and convert back to bytes
        let data_bits = &all_bits[start_bit..(start_bit + bits_needed)];
        let data = Self::bits_to_bytes(data_bits);

        Ok(Some(data))
    }

    /// Convert bytes to a bit array
    fn bytes_to_bits(bytes: &[u8]) -> Vec<bool> {
        let mut bits = Vec::with_capacity(bytes.len() * 8);

        for &byte in bytes {
            // Iterate over each bit in the byte, from MSB (Most Significant Bit) to LSB (Lest Significant Bit)
            for i in (0..8).rev() {
                // Extract and push the bit
                bits.push((byte >> i) & 1 == 1);
            }
        }

        bits
    }

    /// Convert bits to bytes
    fn bits_to_bytes(bits: &[bool]) -> Vec<u8> {
        let byte_count = bits.len().div_ceil(8);
        let mut bytes = vec![0u8; byte_count];

        for (i, &bit) in bits.iter().enumerate() {
            if bit {
                // Get byte index and bit position, to flip the correct bit
                let byte_idx = i / 8;
                let bit_pos = 7 - (i % 8); // MSB (Most Significant Bit) ordering

                // Flip the bit to 1
                bytes[byte_idx] |= 1 << bit_pos;
            }
        }

        bytes
    }

    /// Convert bitstream to field elements
    fn bits_to_field_elements(bits: &[bool]) -> Vec<[u8; 32]> {
        let mut result = Vec::new();

        // Process the bits in chunks of 254 bits (31.75 bytes)
        let start_bit = Self::UNALLOCATED_BITS_PER_FE; // Skip the top 2 bits in each FE, as per KZG commitment requirements
        for chunk in bits.chunks(USABLE_BITS_PER_FIELD_ELEMENT) {
            let mut fe = [0u8; FIELD_ELEMENT_BYTES_USIZE];

            // Process each bit in the chunk
            for (i, &bit) in chunk.iter().enumerate() {
                if bit {
                    // Calculate byte index (0-31) and bit position (0-7)
                    let byte_idx = (start_bit + i) / 8;
                    let bit_pos = 7 - ((start_bit + i) % 8); // MSB (Most Significant Bit) ordering

                    // Flip the bit to 1
                    fe[byte_idx] |= 1 << bit_pos;
                }
            }

            result.push(fe);
        }

        result
    }

    /// Extract bits from field elements
    fn field_elements_to_bits(fes: &[[u8; 32]]) -> Vec<bool> {
        let mut bits = Vec::with_capacity(fes.len() * USABLE_BITS_PER_FIELD_ELEMENT);

        for fe in fes {
            // Process each byte of the field element
            for (byte_idx, &byte) in fe.iter().enumerate() {
                // For the first byte, only use the lower 2 bits
                let start_bit = if byte_idx == 0 {
                    Self::UNALLOCATED_BITS_PER_FE
                } else {
                    0
                };

                // Extract bits from the byte
                for bit_pos in start_bit..8 {
                    bits.push((byte >> (7 - bit_pos)) & 1 == 1);
                }
            }
        }

        bits
    }
}

impl SidecarCoder for OptimizedBlobCoder {
    fn required_fe(&self, data: &[u8]) -> usize {
        let mut total_bits = 0;

        // Reserve space for the header
        if !self.header_prepended {
            total_bits += USABLE_BITS_PER_FIELD_ELEMENT;
        }

        // Reserve space for the length prefix
        if self.prepend_length {
            total_bits += Self::LENGTH_PREFIX_BITS;
        }

        // Reserve space for the data
        total_bits += data.len() * 8;

        // We can fit data into 31.75 bytes per FE, or 254 bits.
        total_bits.div_ceil(USABLE_BITS_PER_FIELD_ELEMENT)
    }

    fn code(&mut self, builder: &mut PartialSidecar, data: &[u8]) {
        // Prepend header only once
        if !self.header_prepended {
            let mut header_bits = Self::bytes_to_bits(self.get_name().as_bytes());

            // Resize header, so it is exactly 254 bits (31.75 bytes) long
            header_bits.resize(USABLE_BITS_PER_FIELD_ELEMENT, false);

            let fe = Self::bits_to_field_elements(&header_bits)[0];
            builder.ingest_valid_fe(WholeFe::new(&fe).unwrap());
            self.header_prepended = true;
        }

        if data.is_empty() {
            return;
        }

        let mut data_bits = vec![];
        // Prepend length prefix if required
        if self.prepend_length {
            // Create length prefix (32 bits/4 bytes)
            let length_prefix = (data.len() as u32).to_be_bytes();

            // Convert data to bits, prepended with length
            data_bits = Self::bytes_to_bits(&length_prefix);
        }

        // Append data
        data_bits.extend(Self::bytes_to_bits(data));

        // Convert bits to field elements
        let fes = Self::bits_to_field_elements(&data_bits);

        // Ingest each field element
        for fe in fes {
            builder.ingest_valid_fe(WholeFe::new(&fe).unwrap());
        }
    }

    fn finish(self, _builder: &mut PartialSidecar) {}

    fn decode_all(&mut self, blobs: &[Blob]) -> Option<Vec<Vec<u8>>> {
        if blobs.is_empty() {
            return None;
        }

        // Validate all field elements
        if blobs
            .iter()
            .flat_map(|blob| blob.chunks(FIELD_ELEMENT_BYTES_USIZE).map(WholeFe::new))
            .any(|fe| fe.is_none())
        {
            return None;
        }

        let mut fes = blobs.iter().flat_map(|blob| {
            blob.chunks(FIELD_ELEMENT_BYTES_USIZE)
                .map(WholeFe::new)
                .map(|fe| fe.unwrap())
        });

        // Consume header FE
        fes.next().unwrap();

        let mut res = Vec::new();
        loop {
            match self.decode_one(&mut fes) {
                Ok(Some(data)) => res.push(data),
                Ok(None) => break,
                Err(()) => return None,
            }
        }

        Some(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip4844::builder::SidecarBuilder;
    use alloy_eips::eip4844::USABLE_BYTES_PER_BLOB;

    #[test]
    fn test_bits_conversion() {
        let original = b"Hello, world!";
        let bits = OptimizedBlobCoder::bytes_to_bits(original);
        let bytes = OptimizedBlobCoder::bits_to_bytes(&bits);
        assert_eq!(bytes, original);
    }

    #[test]
    fn test_field_element_conversion() {
        let original = vec![true, false, true, true, false, false, true, false];
        let fes = OptimizedBlobCoder::bits_to_field_elements(&original);
        let bits = OptimizedBlobCoder::field_elements_to_bits(&fes);

        // We'll have padding bits, so just check the first 8 bits
        assert_eq!(bits[0..8], original);
    }

    #[test]
    fn test_header_name_encoding() {
        let mut coder = OptimizedBlobCoder::new(true);
        let mut builder = SidecarBuilder::from_coder_and_data(coder, &vec![0]);

        let blobs = builder.take();
        let header_fe = blobs[0].as_slice()[0..32].to_vec();
        let header_bits = OptimizedBlobCoder::field_elements_to_bits(&[header_fe.try_into().unwrap()]);
        let header_bytes = OptimizedBlobCoder::bits_to_bytes(&header_bits)
            .into_iter()
            .take_while(|&b| b != 0)
            .collect::<Vec<_>>();
        let header_name = String::from_utf8_lossy(&header_bytes);

        assert_eq!(header_name, coder.get_name());
    }

    #[test]
    fn test_various_data_sizes() {
        // Test with different data sizes to ensure we handle edge cases correctly
        let test_sizes = [
            0, 1, 30, 31, 32, 33, 63, 64, 65, 127, 128, 129, 252, 253, 254, 255, 256, 257, 1000,
            4096, 8192,
        ];

        for size in test_sizes {
            let data = vec![255u8; size];
            let mut coder = OptimizedBlobCoder::new(true);
            let builder = SidecarBuilder::from_coder_and_data(coder, &data);

            let blobs = builder.take();
            let decoded = coder.decode_all(&blobs).unwrap();

            assert_eq!(decoded.len(), if size == 0 { 0 } else { 1 });
            if size > 0 {
                assert_eq!(decoded[0], data);
            }
        }
    }

    #[test]
    fn test_large_data() {
        let data = vec![255u8; USABLE_BYTES_PER_BLOB + 1000];
        let mut coder = OptimizedBlobCoder::new(true);
        let builder = SidecarBuilder::from_coder_and_data(coder, &data);

        let blobs = builder.take();
        let decoded = coder.decode_all(&blobs).unwrap().concat();

        assert_eq!(decoded, data);
    }

    #[test]
    fn test_31_75_byte_vector_of_ones_encodes_into_one_fe_no_prepended_length() {
        // Create a 31.75-byte vector with all bits set to 1
        let mut data = vec![255u8; 31];
        // Set the last byte to 252 (11111100 in binary)
        data.push(252u8);

        let mut coder = OptimizedBlobCoder::new(false);
        let builder = SidecarBuilder::from_coder_and_data(coder, &data);
        let blobs = builder.take();

        let decoded = coder.decode_all(&blobs);

        // Check that only first 32 bytes of data are non-zero (skip header)
        let first_32_bytes = blobs[0].as_slice()[32..64].to_vec();
        assert_eq!(first_32_bytes[0], 63); // First byte should be 63 (00111111)
        assert_eq!(first_32_bytes[1..32], vec![255; 31]); // All other bytes should be 255

        // All other bytes should be zero
        for byte in blobs[0].as_slice()[64..].iter() {
            assert_eq!(*byte, 0);
        }
    }

    #[test]
    fn test_max_data_size_no_prepended_length() {
        let max_usable_bytes = USABLE_BYTES_PER_BLOB - 32; // Subtract 32 bytes for the header
        let data = vec![255u8; max_usable_bytes];
        let mut coder = OptimizedBlobCoder::new(false);
        let builder = SidecarBuilder::from_coder_and_data(coder, &data);

        let blobs = builder.take();
        let decoded = coder.decode_all(&blobs).unwrap().concat();

        assert_eq!(decoded, data);
    }

    #[test]
    fn test_blob_overflow_no_prepended_length() {
        let max_usable_bytes = USABLE_BYTES_PER_BLOB - 32; // Subtract 32 bytes for the header
        let overflow_bytes_count = 150;
        let data = vec![255u8; max_usable_bytes + overflow_bytes_count];
        let mut coder = OptimizedBlobCoder::new(false);
        let builder = SidecarBuilder::from_coder_and_data(coder, &data);

        let blobs = builder.take();
        let decoded = coder.decode_all(&blobs).unwrap().concat();

        // Add 32 for the header (present only in the first FE)
        // Subtract bytes of data that overflowed
        let zeros_count = max_usable_bytes + 32 - overflow_bytes_count;

        assert_eq!(decoded[0..data.len()], data);
        assert_eq!(decoded[data.len()..], vec![0u8; zeros_count]);
    }
}
