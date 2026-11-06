// Copyright (c) 2022-2026 Alex Chi Z
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![allow(unused_variables)] // TODO(you): remove this lint after implementing this mod
#![allow(dead_code)] // TODO(you): remove this lint after implementing this mod

use std::{cmp::Ordering, mem};

use bytes::BufMut;

use crate::key::{KeySlice, KeyVec};

use super::Block;

pub(crate) const SIZEOF_U16: usize = mem::size_of::<u16>();

/// Builds a block.
pub struct BlockBuilder {
    /// Offsets of each key-value entries.
    offsets: Vec<u16>,
    /// All serialized key-value pairs in the block.
    data: Vec<u8>,
    /// The expected block size.
    block_size: usize,
    /// The first key in the block
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

    fn compute_overlap(first_key: KeySlice, curr_key: KeySlice) -> usize {
        first_key
            .raw_ref()
            .iter()
            .zip(curr_key.raw_ref())
            .take_while(|(a, b)| a == b)
            .count()
    }

    /// Returns the size of the current block.
    pub fn estimated_size(&self) -> usize {
        self.data.len() + self.offsets.len() * SIZEOF_U16 + SIZEOF_U16
    }

    /// Adds a key-value pair to the block. Returns false when the block is full.
    /// You may find the `bytes::BufMut` trait useful for manipulating binary data.
    #[must_use]
    pub fn add(&mut self, key: KeySlice, value: &[u8]) -> bool {
        assert!(!key.is_empty(), "key cannot be empty");

        let size_estimate = self.estimated_size() + key.len() + value.len() + SIZEOF_U16 * 3;
        if !self.data.is_empty() && size_estimate > self.block_size {
            return false;
        }
        let Ok(data_offset) = u16::try_from(self.data.len()) else {
            return false;
        };
        let Ordering::Less = key.len().cmp(&u16::MAX.into()) else {
            return false;
        };
        let Ok(value_len) = u16::try_from(value.len()) else {
            return false;
        };
        self.offsets.push(data_offset);

        let overlap_len = Self::compute_overlap(self.first_key.as_key_slice(), key);

        self.data.put_u16(overlap_len.try_into().unwrap());
        self.data
            .put_u16((key.len() - overlap_len).try_into().unwrap());
        self.data.put(&key.raw_ref()[overlap_len..]);

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
        assert!(!self.is_empty(), "SsTable cannot contain empty blocks");
        Block {
            data: self.data,
            offsets: self.offsets,
        }
    }
}
