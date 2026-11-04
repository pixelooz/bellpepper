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

use std::sync::Arc;

use anyhow::Result;

use super::SsTable;
use crate::{block::BlockIterator, iterators::StorageIterator, key::KeySlice};

/// An iterator over the contents of an SSTable.
pub struct SsTableIterator {
    table: Arc<SsTable>,
    block_iter: BlockIterator,
    block_idx: usize,
}

impl SsTableIterator {
    /// Create a new iterator and seek to the first key-value pair in the first data block.
    pub fn create_and_seek_to_first(table: Arc<SsTable>) -> Result<Self> {
        let block = table.read_block_cached(0)?;
        let block_iter = BlockIterator::create_and_seek_to_first(block);
        Ok(Self {
            table,
            block_iter,
            block_idx: 0,
        })
    }

    /// Seek to the first key-value pair in the first data block.
    pub fn seek_to_first(&mut self) -> Result<()> {
        let block = self.table.read_block_cached(0)?;
        let block_iter = BlockIterator::create_and_seek_to_first(block);
        self.block_idx = 0;
        self.block_iter = block_iter;
        Ok(())
    }

    fn seek_to_key_inner(table: &Arc<SsTable>, key: KeySlice) -> Result<(usize, BlockIterator)> {
        let mut block_idx = table.find_block_idx(key);
        let block = table.read_block_cached(block_idx)?;
        let mut block_iter = BlockIterator::create_and_seek_to_key(block, key);
        if !block_iter.is_valid() {
            block_idx += 1;
            if block_idx < table.num_of_blocks() {
                let block = table.read_block_cached(block_idx)?;
                block_iter = BlockIterator::create_and_seek_to_first(block);
            }
        }
        Ok((block_idx, block_iter))
    }

    /// Create a new iterator and seek to the first key-value pair which >= `key`.
    pub fn create_and_seek_to_key(table: Arc<SsTable>, key: KeySlice) -> Result<Self> {
        let (block_idx, block_iter) = Self::seek_to_key_inner(&table, key)?;
        Ok(Self {
            table,
            block_iter,
            block_idx: 0,
        })
    }

    /// Seek to the first key-value pair which >= `key`.
    /// Note: You probably want to review the handout for detailed explanation when implementing
    /// this function.
    pub fn seek_to_key(&mut self, key: KeySlice) -> Result<()> {
        let (idx, iter) = Self::seek_to_key_inner(&self.table, key)?;
        self.block_idx = idx;
        self.block_iter = iter;
        Ok(())
    }
}

impl StorageIterator for SsTableIterator {
    type KeyType<'a> = KeySlice<'a>;

    fn value(&self) -> &[u8] {
        self.block_iter.value()
    }

    fn key(&self) -> KeySlice<'_> {
        self.block_iter.key()
    }

    fn is_valid(&self) -> bool {
        self.block_iter.is_valid()
    }

    fn next(&mut self) -> Result<()> {
        self.block_iter.next();
        if !self.block_iter.is_valid() {
            self.block_idx += 1;
            if self.block_idx < self.table.num_of_blocks() {
                self.block_iter = BlockIterator::create_and_seek_to_first(
                    self.table.read_block_cached(self.block_idx)?,
                );
            }
        }
        Ok(())
    }
}
