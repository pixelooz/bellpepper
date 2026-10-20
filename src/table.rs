pub(crate) mod builder;
pub(crate) mod iterator;

use std::{fs::File, io::Write, mem, os::unix::fs::FileExt, path::Path, sync::Arc};

use anyhow::{Context, Result, anyhow, bail};
use bytes::{Buf, BufMut};

use crate::{
    block::Block,
    key::{KeyBytes, KeySlice},
    lsm_storage::BlockCache,
};

/// The Metadata for each block being encoded in an SST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockMeta {
    /// Offset of this data block.
    pub offset: usize,

    /// The first key of this data block.
    pub first_key: KeyBytes,

    /// The last key of this data block.
    pub last_key: KeyBytes,
}

impl BlockMeta {
    /// Encode block meta to a buffer.
    pub fn encode_block_meta(block_meta: &[BlockMeta], buffer: &mut Vec<u8>) -> Result<()> {
        let mut estimated_size = 0;

        for bm in block_meta {
            estimated_size += mem::size_of::<u32>();
            estimated_size += mem::size_of::<u16>();
            estimated_size += bm.first_key.len();
            estimated_size += mem::size_of::<u16>();
            estimated_size += bm.last_key.len();
        }
        buffer.reserve(estimated_size);
        for bm in block_meta {
            buffer.put_u32(u32::try_from(bm.offset).context("SST meta block offset too large")?);
            buffer.put_u16(u16::try_from(bm.first_key.len()).context("SST first key too large")?);
            buffer.put_slice(bm.first_key.raw_ref());
            buffer.put_u16(u16::try_from(bm.last_key.len()).context("SST last key too large")?);
            buffer.put_slice(bm.last_key.raw_ref());
        }
        Ok(())
    }

    /// Decode block meta from a buffer.
    pub fn decode_block_meta(mut buffer: impl Buf) -> Result<Vec<BlockMeta>> {
        let mut block_metas = Vec::new();
        while buffer.has_remaining() {
            let offset = buffer.get_u32() as usize;

            let key_len = buffer.get_u16();
            let first_key = KeyBytes::from_bytes(buffer.copy_to_bytes(key_len as usize));

            let key_len = buffer.get_u16();
            let last_key = KeyBytes::from_bytes(buffer.copy_to_bytes(key_len as usize));

            block_metas.push(BlockMeta {
                offset,
                first_key,
                last_key,
            });
        }
        Ok(block_metas)
    }
}

/// Represents the underlying file and its cached size.
#[derive(Debug)]
pub struct FileObject(Option<File>, u64);

impl FileObject {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Self(Some(file), len))
    }

    pub fn read(&self, offset: u64, len: u64) -> Result<Vec<u8>> {
        let len = usize::try_from(len).context("conversion u64 -> usize failed")?;
        let mut data = vec![0; len];
        match self.0.as_ref() {
            Some(file) => file.read_exact_at(&mut data, offset)?,
            None => bail!("file should exist when calling this method"),
        }
        Ok(data)
    }

    pub fn size(&self) -> u64 {
        self.1
    }

    pub fn create_write(path: &Path, data: Vec<u8>) -> Result<Self> {
        let mut file = File::create(path)?;
        file.write_all(&data)?;
        file.sync_all()?;
        Ok(Self(Some(File::open(path)?), data.len() as u64))
    }
}

/// An SSTable: Collections of Blocks with a footer containing Block metadata used for
/// efficient reads.
#[derive(Debug)]
pub struct SsTable {
    /// The actual storage unit of the [SsTable].
    pub(crate) file: FileObject,

    /// The meta blocks that hold the info for the data blocks.
    pub(crate) block_metas: Vec<BlockMeta>,

    /// The offset that indicates the starting point for meta blocks in the file.
    pub(crate) meta_block_offset: usize,

    id: usize,
    block_cache: Option<Arc<BlockCache>>,

    first_key: KeyBytes,
    last_key: KeyBytes,
    // bloom and timestamps
}

impl SsTable {
    /// The first key of the `SsTable`.
    pub fn first_key(&self) -> &KeyBytes {
        &self.first_key
    }

    /// The last key of the `SsTable`.
    pub fn last_key(&self) -> &KeyBytes {
        &self.last_key
    }

    /// Get number of data blocks.
    pub fn num_of_blocks(&self) -> usize {
        self.block_metas.len()
    }

    /// The id of the current `SsTable`.
    pub fn sst_id(&self) -> usize {
        self.id
    }

    /// Returns the size of the `SsTable` file.
    pub fn table_size(&self) -> u64 {
        self.file.1
    }

    #[cfg(test)]
    pub(crate) fn open_for_test(file: FileObject) -> Result<Self> {
        Self::open(0, None, file)
    }
}

impl SsTable {
    /// Open SSTable from a file.
    pub fn open(id: usize, block_cache: Option<Arc<BlockCache>>, file: FileObject) -> Result<Self> {
        let file_len = file.size();
        let raw_bm_offset = file.read(file_len - 4, 4)?;
        let block_meta_offset = u64::from((&raw_bm_offset[..]).get_u32());
        let raw_block_meta = file.read(block_meta_offset, file_len - 4 - block_meta_offset)?;
        let block_metas = BlockMeta::decode_block_meta(&raw_block_meta[..])?;
        Ok(Self {
            first_key: block_metas.first().unwrap().first_key.clone(),
            last_key: block_metas.last().unwrap().last_key.clone(),
            file,
            block_metas,
            meta_block_offset: block_meta_offset as usize,
            id,
            block_cache,
        })
    }

    /// Create a mock SST with only first key + last key metadata
    pub fn create_meta_only(
        id: usize,
        file_size: u64,
        first_key: KeyBytes,
        last_key: KeyBytes,
    ) -> Self {
        Self {
            file: FileObject(None, file_size),
            block_metas: vec![],
            meta_block_offset: 0,
            id,
            block_cache: None,
            first_key,
            last_key,
        }
    }

    /// Read a block from the disk.
    pub fn read_block(&self, block_idx: usize) -> Result<Arc<Block>> {
        let offset = self.block_metas[block_idx].offset as u64;
        let offset_end = self
            .block_metas
            .get(block_idx + 1)
            .map_or(self.meta_block_offset, |bm| bm.offset) as u64;

        let block_with_checksum = self.file.read(offset, offset_end - offset)?;
        let block_len = block_with_checksum.len() - 4;

        let block = &block_with_checksum[..block_len];
        let checksum = (&block_with_checksum[block_len..]).get_u32();

        if crc32fast::hash(block) != checksum {
            bail!("block checksum mismatched")
        }
        Ok(Arc::new(Block::decode(block)))
    }

    /// Read a block from disk, with block cache.
    pub fn read_block_cached(&self, block_idx: usize) -> Result<Arc<Block>> {
        if let Some(block_cache) = &self.block_cache {
            block_cache
                .try_get_with((self.id, block_idx), || self.read_block(block_idx))
                .map_err(|err| anyhow!(err))
        } else {
            self.read_block(block_idx)
        }
    }

    /// Find the block that may contain `key`.
    pub fn find_block_idx(&self, key: KeySlice) -> usize {
        self.block_metas
            .partition_point(|bm| bm.first_key.as_key_slice() <= key)
            .saturating_sub(1)
    }
}
