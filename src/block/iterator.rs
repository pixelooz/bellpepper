use std::{cmp::Ordering, sync::Arc};

use bytes::Buf;

use crate::{
    block::{Block, SIZEOF_U16},
    key::{KeySlice, KeyVec},
};

/// Iterates on a `Block`.
pub struct BlockIterator {
    /// The internal `Block` wrapper by an Arc.
    block: Arc<Block>,

    /// The current key, empty represents the iterator is invalid.
    key: KeyVec,

    /// The current value range in `block.data`, corresponds to the current key.
    value_range: (usize, usize),

    /// Current index of the key-value pair, should be in the range of [0, num of elements].
    index: usize,

    /// The first key in the block.
    first_key: KeyVec,
}

impl Block {
    fn get_first_key(&self) -> KeyVec {
        let mut buffer = &self.data[..];
        buffer.get_u16(); // skipping the overlap bytes
        let key_len = usize::from(buffer.get_u16());
        let key = &buffer[..key_len];
        KeyVec::from_vec(key.to_vec())
    }
}

impl BlockIterator {
    /// Returns a new BlockIterator instantiated with the given Arc<Block>.
    fn new(block: Arc<Block>) -> Self {
        Self {
            first_key: block.get_first_key(),
            block,
            key: KeyVec::new(),
            index: 0,
            value_range: (0, 0),
        }
    }

    /// Creates a block iterator and seek to the first entry.
    pub fn create_and_seek_to_first(block: Arc<Block>) -> Self {
        let mut iter = Self::new(block);
        iter.seek_to_first();
        iter
    }

    /// Creates a block iterator and seek to the first key that is greater than or equal
    /// to the `search_key`.
    pub fn create_and_seek_to_key(block: Arc<Block>, search_key: KeySlice) -> Self {
        let mut iter = Self::new(block);
        iter.seek_to_key(search_key);
        iter
    }

    /// Returns the key of the current entry.
    pub fn key(&self) -> KeySlice<'_> {
        self.key.as_key_slice()
    }

    /// Returns the value of the current entry.
    pub fn value(&self) -> &[u8] {
        &self.block.data[self.value_range.0..self.value_range.1]
    }

    /// Returns true if the iterator is valid.
    /// Note: You may want to make use of `key`
    pub fn is_valid(&self) -> bool {
        !self.key.is_empty()
    }

    /// Seeks to the first key in the block.
    pub fn seek_to_first(&mut self) {
        self.seek_to(0);
    }

    /// Move to the next key in the block.
    pub fn next(&mut self) {
        self.index += 1;
        self.seek_to(self.index);
    }

    /// Seek to the first key that is greater than or equal to (>=) `search_key`.
    pub fn seek_to_key(&mut self, search_key: KeySlice) {
        let mut high = self.block.offsets.len();
        let mut low = 0;
        while low < high {
            let mid = low + (high - low) / 2;
            self.seek_to(mid);
            match self.key().cmp(&search_key) {
                Ordering::Less => low = mid + 1,
                Ordering::Equal => return,
                Ordering::Greater => high = mid,
            }
        }
        self.seek_to(low);
    }

    /// Seeks to the entry located at offset[index] and populates the `key` and `value_range`
    /// and updates the index for the current key value pair.
    fn seek_to(&mut self, index: usize) {
        if index >= self.block.offsets.len() {
            self.key.clear();
            self.value_range = (0, 0);
            return;
        }
        let offset = usize::from(self.block.offsets[index]);
        self.seek_to_offset(offset);
        self.index = index;
    }

    /// Seeks to the entry located at offset and populates the `key` and `value_range`
    /// and updates the index for the current key value pair.
    fn seek_to_offset(&mut self, offset: usize) {
        let mut entries = &self.block.data[offset..];
        let overlap_len = usize::from(entries.get_u16());
        let rest_key_len = usize::from(entries.get_u16());
        let rest_key = &entries[..rest_key_len];
        self.key.clear();
        self.key.append(&self.first_key.raw_ref()[..overlap_len]);
        self.key.append(rest_key);
        entries.advance(rest_key_len);
        let value_len = usize::from(entries.get_u16());
        let value_offset_begin = offset + SIZEOF_U16 * 3 + rest_key_len;
        let value_offset_end = value_offset_begin + value_len;
        self.value_range = (value_offset_begin, value_offset_end);
        entries.advance(value_len);
    }
}
