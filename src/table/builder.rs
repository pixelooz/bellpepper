use std::{mem, path::Path, sync::Arc};

use anyhow::{Context, Result};
use bytes::BufMut;

use crate::{
    block::builder::BlockBuilder,
    key::{KeySlice, KeyVec},
    lsm_storage::BlockCache,
    table::{BlockMeta, FileObject, SsTable},
};

/// Builds an [SsTable] from key value pairs.
pub struct SsTableBuilder {
    /// The active 4KB block.
    builder: BlockBuilder,

    /// The first key of the active block.
    first_key: KeyVec,

    /// The last key of the active block.
    last_key: KeyVec,
    data: Vec<u8>,
    pub(crate) block_metas: Vec<BlockMeta>,
    block_size: usize,

    key_hashes: Vec<u32>,
}

impl SsTableBuilder {
    /// Create a builder based on target block size.
    pub fn new(block_size: usize) -> Self {
        Self {
            builder: BlockBuilder::new(block_size),
            first_key: KeyVec::new(),
            last_key: KeyVec::new(),
            data: Vec::with_capacity(2 << 20),
            block_metas: Vec::new(),
            block_size,
            key_hashes: Vec::new(),
        }
    }

    /// Adds a key-value pair to SSTable. Will split a new block when the current block is full.
    pub fn add(&mut self, key: KeySlice, value: &[u8]) {
        if self.first_key.is_empty() {
            self.first_key.set_from_slice(key);
        }
        self.key_hashes.push(farmhash::fingerprint32(key.raw_ref()));
        if self.builder.add(key, value) {
            self.last_key.set_from_slice(key);
            return;
        }
        self.finish_block();
        assert!(
            self.builder.add(key, value),
            "adding to a new block should never fail"
        );
        self.first_key.set_from_slice(key);
        self.last_key.set_from_slice(key);
    }

    /// Replaces the old [BlockBuilder] with a new one and inserts the block meta into the SsTable
    /// while computing and extending the crc32 checksum for the block.
    fn finish_block(&mut self) {
        let old_builder = mem::replace(&mut self.builder, BlockBuilder::new(self.block_size));
        let enc_block = old_builder.build().encode();
        let block_meta = BlockMeta {
            offset: self.data.len(),
            first_key: mem::take(&mut self.first_key).into_key_bytes(),
            last_key: mem::take(&mut self.last_key).into_key_bytes(),
        };
        let checksum = crc32fast::hash(&enc_block);
        self.block_metas.push(block_meta);
        self.data.extend(enc_block);
        self.data.put_u32(checksum);
    }

    /// Get the estimated size of the SSTable.
    ///
    /// Since the data blocks contain much more data than meta blocks, just returns the size
    /// of data blocks here.
    pub fn estimated_size(&self) -> usize {
        self.data.len()
    }

    /// Builds the SSTable and writes it to the given path. Use the `FileObject` structure
    /// to manipulate the disk objects.
    pub fn build(
        mut self,
        path: impl AsRef<Path>,
        id: usize,
        block_cache: Option<Arc<BlockCache>>,
    ) -> Result<SsTable> {
        self.finish_block();
        let mut buffer = self.data;
        let meta_offset = buffer.len();

        BlockMeta::encode_block_meta(&self.block_metas, &mut buffer)?;
        buffer.put_u32(u32::try_from(meta_offset).context("meta_offset is too large")?);

        let file = FileObject::create_write(path.as_ref(), buffer)?;
        Ok(SsTable {
            first_key: self.block_metas.first().unwrap().first_key.clone(),
            last_key: self.block_metas.last().unwrap().last_key.clone(),
            file,
            block_metas: self.block_metas,
            meta_block_offset: meta_offset,
            id,
            block_cache,
        })
    }

    #[cfg(test)]
    pub(crate) fn build_for_test(self, path: impl AsRef<Path>) -> Result<SsTable> {
        self.build(path, 0, None)
    }
}
