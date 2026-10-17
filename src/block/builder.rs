use bytes::BufMut;

use crate::{
    block::{Block, SIZEOF_U16},
    key::{KeySlice, KeyVec},
};

/// Builds a Block.
pub(crate) struct BlockBuilder {
    /// Offsets of each key-value entries.
    offsets: Vec<u16>,

    /// All serialized key-value pairs in the block.
    data: Vec<u8>,

    /// Estimated size of the data.
    block_size: usize,

    /// The first key in the block.
    first_key: KeyVec,
}

impl BlockBuilder {
    /// Creates a new block builder.
    pub fn new(block_size: usize) -> Self {
        Self {
            offsets: Vec::new(),
            data: Vec::new(),
            block_size,
            first_key: KeyVec::new(),
        }
    }

    /// Calculates the approx size of the block.
    pub fn estimated_size(&self) -> usize {
        SIZEOF_U16 + self.offsets.len() * SIZEOF_U16 + self.data.len()
    }

    /// Adds a key-value pair to the block. Returns false when the block is full.
    #[must_use]
    pub fn add(&mut self, key: KeySlice, value: &[u8]) -> bool {
        assert!(!key.is_empty(), "key cannot be empty");

        let Ok(key_len) = u16::try_from(key.len()) else {
            return false;
        };
        let Ok(value_len) = u16::try_from(value.len()) else {
            return false;
        };
        let entry_len = key
            .len()
            .saturating_add(value.len())
            .saturating_add(SIZEOF_U16 * 2);

        let is_block_full = self.estimated_size().saturating_add(entry_len) > self.block_size;
        let is_offset_full = self.data.len() > usize::from(u16::MAX);
        let is_count_full = self.offsets.len() >= usize::from(u16::MAX);

        if !self.is_empty() && (is_block_full || is_offset_full || is_count_full) {
            return false;
        }
        let Ok(offset) = u16::try_from(self.data.len()) else {
            return false;
        };
        self.offsets.push(offset);
        self.data.put_u16(key_len);
        self.data.put(key.raw_ref());
        self.data.put_u16(value_len);
        self.data.put(value);
        if self.first_key.is_empty() {
            self.first_key = key.to_key_vec();
        }
        true
    }

    /// Check if there is no key-value pair in the block.
    pub fn is_empty(&self) -> bool {
        self.offsets.is_empty()
    }

    /// Finalize the block.
    pub fn build(self) -> Block {
        assert!(!self.is_empty(), "block should not be empty");
        Block {
            data: self.data,
            offsets: self.offsets,
        }
    }
}
