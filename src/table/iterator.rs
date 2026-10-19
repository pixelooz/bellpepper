use anyhow::Result;
use std::{cmp::Ordering, sync::Arc};

use crate::{
    block::{builder::BlockBuilder, iterator::BlockIterator},
    iterators::StorageIterator,
    key::KeySlice,
    table::SsTable,
};

/// An iterator over the contents of the [SsTable].
pub struct SsTableIterator {
    table: Arc<SsTable>,
    block_iterator: BlockIterator,
    block_index: usize,
}

impl SsTableIterator {
    /// Create a new iterator and seek to the first key-value pair in the first data block.
    pub fn create_and_seek_to_first(table: Arc<SsTable>) -> Result<Self> {
        let (index, block_iterator) = Self::seek_to_first_inner(&table)?;
        Ok(Self {
            table,
            block_iterator,
            block_index: index,
        })
    }

    /// Seek to the first key-value pair in the first data block.
    pub fn seek_to_first(&mut self) -> Result<()> {
        let (index, block_iterator) = Self::seek_to_first_inner(&self.table)?;
        self.block_index = index;
        self.block_iterator = block_iterator;
        Ok(())
    }

    fn seek_to_first_inner(table: &Arc<SsTable>) -> Result<(usize, BlockIterator)> {
        let block = table.read_block_cached(0)?;
        let mut block_iterator = BlockIterator::create_and_seek_to_first(block);
        Ok((0, block_iterator))
    }

    /// Create a new iterator and seek to the first key-value pair which >= `key`.
    pub fn create_and_seek_to_key(table: Arc<SsTable>, search_key: KeySlice) -> Result<Self> {
        let (index, block_iterator) = Self::seek_to_key_inner(&table, search_key)?;
        Ok(Self {
            table,
            block_iterator,
            block_index: index,
        })
    }

    /// Seek to the first key-value pair which >= `key`.
    /// Note: You probably want to review the handout for detailed explanation when implementing
    /// this function.
    pub fn seek_to_key(&mut self, search_key: KeySlice) -> Result<()> {
        let (index, block_iterator) = Self::seek_to_key_inner(&self.table, search_key)?;
        self.block_index = index;
        self.block_iterator = block_iterator;
        Ok(())
    }

    fn seek_to_key_inner(
        table: &Arc<SsTable>,
        search_key: KeySlice,
    ) -> Result<(usize, BlockIterator)> {
        let mut block_idx = table.find_block_idx(search_key);
        let mut block = table.read_block_cached(block_idx)?;
        let mut block_iterator = BlockIterator::create_and_seek_to_key(block, search_key);
        if !block_iterator.is_valid() {
            block_idx += 1;
            if block_idx < table.num_of_blocks() {
                block = table.read_block_cached(block_idx)?;
                block_iterator = BlockIterator::create_and_seek_to_first(block);
            }
        }
        Ok((block_idx, block_iterator))
    }
}

impl StorageIterator for SsTableIterator {
    type KeyType<'a> = KeySlice<'a>;

    /// Return the `key` that's held by the underlying block iterator.
    fn key(&self) -> KeySlice<'_> {
        self.block_iterator.key()
    }

    /// Return the `value` that's held by the underlying block iterator.
    fn value(&self) -> &[u8] {
        self.block_iterator.value()
    }

    /// Return whether the current block iterator is valid or not.
    fn is_valid(&self) -> bool {
        self.block_iterator.is_valid()
    }

    /// Move to the next `key` in the block.
    /// Note: You may want to check if the current block iterator is valid after the move.
    fn next(&mut self) -> Result<()> {
        self.block_iterator.next();
        if self.block_iterator.is_valid() {
            return Ok(());
        }
        self.block_index += 1;
        if self.block_index < self.table.num_of_blocks() {
            let block = self.table.read_block_cached(self.block_index)?;
            self.block_iterator = BlockIterator::create_and_seek_to_first(block);
        }
        Ok(())
    }
}
