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

pub(crate) mod bloom;
mod builder;
mod iterator;

use std::fs::File;
use std::mem;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, ensure};
pub use builder::SsTableBuilder;
use bytes::{Buf, BufMut};
pub use iterator::SsTableIterator;

use crate::block::Block;
use crate::key::{KeyBytes, KeySlice};
use crate::lsm_storage::BlockCache;

use self::bloom::Bloom;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockMeta {
    /// Offset of this data block.
    pub offset: usize,
    /// The first key of the data block.
    pub first_key: KeyBytes,
    /// The last key of the data block.
    pub last_key: KeyBytes,
}

impl BlockMeta {
    /// Encode block meta to a buffer.
    /// You may add extra fields to the buffer,
    /// in order to help keep track of `first_key` when decoding from the same buffer in the future.
    pub fn encode_block_meta(block_metas: &[BlockMeta], buffer: &mut Vec<u8>) {
        let mut estimated_size = block_metas.len();
        for meta in block_metas {
            estimated_size += mem::size_of::<u32>();
            estimated_size += mem::size_of::<u16>();
            estimated_size += meta.first_key.len();
            estimated_size += mem::size_of::<u16>();
            estimated_size += meta.last_key.len();
        }
        estimated_size += mem::size_of::<u32>();
        buffer.reserve(estimated_size);
        let original_len = buffer.len();
        buffer.put_u32(block_metas.len().try_into().unwrap());
        for meta in block_metas {
            buffer.put_u32(meta.offset.try_into().unwrap());
            buffer.put_u16(meta.first_key.len().try_into().unwrap());
            buffer.put(meta.first_key.raw_ref());
            buffer.put_u16(meta.last_key.len().try_into().unwrap());
            buffer.put(meta.last_key.raw_ref());
        }
        buffer.put_u32(crc32fast::hash(&buffer[original_len + 4..]));
    }

    /// Decode block meta from a buffer.
    pub fn decode_block_meta(mut buffer: &[u8]) -> Result<Vec<BlockMeta>> {
        let length = buffer.get_u32().try_into().unwrap();
        let mut block_metas = Vec::with_capacity(length);
        let checksum = crc32fast::hash(&buffer[..buffer.len() - 4]);
        for _ in 0..length {
            let offset = buffer.get_u32();
            let first_key_len = buffer.get_u16().into();
            let first_key = KeyBytes::from_bytes(buffer.copy_to_bytes(first_key_len));
            let last_key_len = buffer.get_u16().into();
            let last_key = KeyBytes::from_bytes(buffer.copy_to_bytes(last_key_len));
            block_metas.push(BlockMeta {
                offset: offset.try_into().unwrap(),
                first_key,
                last_key,
            });
        }
        ensure!(buffer.get_u32() == checksum, "meta checksum mismatched");
        Ok(block_metas)
    }
}

/// A file object.
pub struct FileObject(Option<File>, u64);

impl FileObject {
    pub fn read(&self, offset: u64, len: u64) -> Result<Vec<u8>> {
        let mut data = vec![0; len as usize];
        self.0
            .as_ref()
            .unwrap()
            .read_exact_at(&mut data[..], offset)?;
        Ok(data)
    }

    pub fn size(&self) -> u64 {
        self.1
    }

    /// Create a new file object (day 2) and write the file to the disk (day 4).
    pub fn create_write(path: &Path, data: Vec<u8>) -> Result<Self> {
        std::fs::write(path, &data)?;
        let file = File::open(path)?;
        file.sync_all()?;
        Ok(Self(Some(file), data.len().try_into().unwrap()))
    }

    pub fn open(path: &Path) -> Result<Self> {
        let file = File::options().read(true).write(false).open(path)?;
        let size = file.metadata()?.len();
        Ok(FileObject(Some(file), size))
    }
}

/// An SSTable.
pub struct SsTable {
    /// The actual storage unit of SsTable, the format is as above.
    pub(crate) file: FileObject,
    /// The meta blocks that hold info for data blocks.
    pub(crate) block_meta: Vec<BlockMeta>,
    /// The offset that indicates the start point of meta blocks in `file`.
    pub(crate) block_meta_offset: usize,
    sst_id: usize,
    block_cache: Option<Arc<BlockCache>>,
    first_key: KeyBytes,
    last_key: KeyBytes,
    pub(crate) bloom: Option<Bloom>,
    /// The maximum timestamp stored in this SST, implemented in week 3.
    max_ts: u64,
}

impl SsTable {
    #[cfg(test)]
    pub(crate) fn open_for_test(file: FileObject) -> Result<Self> {
        Self::open(0, None, file)
    }

    /// Open SSTable from a file.
    pub fn open(id: usize, block_cache: Option<Arc<BlockCache>>, file: FileObject) -> Result<Self> {
        let length = file.size();

        let raw_bloom_offset = file.read(length - 4, 4)?;
        let bloom_offset = (&raw_bloom_offset[..]).get_u32().into();
        let raw_bloom = file.read(bloom_offset, length - bloom_offset - 4)?;
        let bloom = Bloom::decode(&raw_bloom)?;

        let raw_meta_offset = file.read(bloom_offset - 4, 4)?;
        let meta_offset = (&raw_meta_offset[..]).get_u32().into();
        let raw_meta = file.read(meta_offset, bloom_offset - meta_offset - 4)?;
        let block_meta = BlockMeta::decode_block_meta(&raw_meta[..])?;

        Ok(Self {
            file,
            block_meta_offset: meta_offset.try_into().expect("still 32bit machine?"),
            sst_id: id,
            block_cache,
            first_key: block_meta.first().unwrap().first_key.clone(),
            last_key: block_meta.last().unwrap().last_key.clone(),
            block_meta,
            bloom: Some(bloom),
            max_ts: 0,
        })
    }

    /// Read a block from the disk.
    pub fn read_block(&self, block_idx: usize) -> Result<Arc<Block>> {
        let offset_begin = self.block_meta[block_idx].offset;
        let offset_end = self
            .block_meta
            .get(block_idx + 1)
            .map_or(self.block_meta_offset, |x| x.offset);
        let range_len = offset_end
            .checked_sub(offset_begin)
            .context("invalid range")?;
        let block_len = range_len.checked_sub(4).context("checksum is truncated")?;
        let raw_block_with_checksum = self.file.read(
            offset_begin.try_into().unwrap(),
            range_len.try_into().unwrap(),
        )?;
        let raw_block = &raw_block_with_checksum[..block_len];
        let checksum = (&raw_block_with_checksum[block_len..]).get_u32();
        ensure!(
            crc32fast::hash(raw_block) == checksum,
            "block checksum mismatched"
        );
        Ok(Arc::new(Block::decode(raw_block)))
    }

    /// Read a block from disk, with block cache. (Day 4)
    pub fn read_block_cached(&self, block_idx: usize) -> Result<Arc<Block>> {
        if let Some(block_cache) = &self.block_cache {
            block_cache
                .try_get_with((self.sst_id, block_idx), || self.read_block(block_idx))
                .map_err(|e| anyhow!("{}", e))
        } else {
            self.read_block(block_idx)
        }
    }

    /// Find the block that may contain `key`.
    /// Note: You may want to make use of the `first_key` stored in `BlockMeta`.
    /// You may also assume the key-value pairs stored in each consecutive block are sorted.
    pub fn find_block_idx(&self, key: KeySlice) -> usize {
        self.block_meta
            .partition_point(|x| x.first_key.as_key_slice() <= key)
            .saturating_sub(1)
    }

    /// Get number of data blocks.
    pub fn num_of_blocks(&self) -> usize {
        self.block_meta.len()
    }

    pub fn sst_id(&self) -> usize {
        self.sst_id
    }

    pub fn table_size(&self) -> u64 {
        self.file.1
    }

    pub fn max_ts(&self) -> u64 {
        self.max_ts
    }

    pub fn first_key(&self) -> &KeyBytes {
        &self.first_key
    }

    pub fn last_key(&self) -> &KeyBytes {
        &self.last_key
    }

    /// Create a mock SST with only first key + last key metadata
    pub fn create_meta_only(
        sst_id: usize,
        file_size: u64,
        first_key: KeyBytes,
        last_key: KeyBytes,
    ) -> Self {
        Self {
            file: FileObject(None, file_size),
            block_meta: vec![],
            block_meta_offset: 0,
            sst_id,
            block_cache: None,
            first_key,
            last_key,
            bloom: None,
            max_ts: 0,
        }
    }
}
