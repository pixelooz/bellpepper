pub(crate) mod builder;

use std::mem;

use anyhow::{Context, Result, ensure};
use bytes::{BufMut, Bytes};

pub(crate) const SIZEOF_U16: usize = mem::size_of::<u16>();

/// A block is the smallest unit of read and caching in LSM tree. It is a collection of sorted key-value pairs.
pub struct Block {
    pub(crate) data: Vec<u8>,
    pub(crate) offsets: Vec<u16>,
}

impl Block {
    /// Encode the internal data to the data layout illustrated in the course
    pub fn encode(&self) -> Bytes {
        let mut buffer = self.data.clone();
        let offset_len = self.offsets.len();
        for offset in &self.offsets {
            buffer.put_u16(*offset);
        }
        buffer.put_u16(offset_len as u16);
        buffer.into()
    }

    /// Decode from the data layout, transform the input `data` to a single `Block`
    pub fn decode(data: &[u8]) -> Self {
        Self::decode_checked(data).expect("invalid block encoding")
    }

    pub(crate) fn decode_checked(data: &[u8]) -> Result<Self> {
        ensure!(data.len() >= SIZEOF_U16, "block footer is truncated");
        let entry_count = u16::from_be_bytes([data[data.len() - 2], data[data.len() - 1]]) as usize;
        ensure!(entry_count > 0, "block and not entries");

        let offset_size = entry_count
            .checked_mul(SIZEOF_U16)
            .context("block offset is too large")?;

        let footer_size = offset_size
            .checked_add(SIZEOF_U16)
            .context("block footer too large")?;

        ensure!(footer_size <= data.len(), "block offset table is truncated");
        let data_end = data.len() - footer_size;
        let offsets_raw = &data[data_end..data.len() - SIZEOF_U16];

        let offsets: Vec<u16> = offsets_raw
            .chunks(SIZEOF_U16)
            .map(|x| u16::from_be_bytes([x[0], x[1]]))
            .collect();

        ensure!(offsets[0] == 0, "first block entry must start at offset 0");
        ensure!(
            offsets.windows(2).all(|pair| pair[0] < pair[1]),
            "block entry offsets are not strictly increasing"
        );
        ensure!(
            offsets.iter().all(|offset| usize::from(*offset) < data_end),
            "block entry offset is outside the data section"
        );
        for (index, offset) in offsets.iter().enumerate() {
            let entry_start = usize::from(*offset);
            let entry_end = offsets
                .get(index + 1)
                .map_or(data_end, |offset| usize::from(*offset));

            let entry = &data[entry_start..entry_end];
            ensure!(entry.len() >= 4, "block entry header is truncated");

            let key_len = usize::from(u16::from_be_bytes([entry[0], entry[1]]));
            let key_end = 2usize
                .checked_add(key_len)
                .context("block key len overflow")?;
            let value_len_end = key_end
                .checked_add(SIZEOF_U16)
                .context("block value header overflow")?;
            ensure!(
                value_len_end <= entry.len(),
                "value_len points outside entry's length"
            );
            let value_len = usize::from(u16::from_be_bytes([entry[key_end], entry[key_end + 1]]));
            let entry_len = value_len_end
                .checked_add(value_len)
                .context("block value length overflowed")?;
            ensure!(entry_len == entry.len(), "block length is invalid");
        }
        let data = data[0..data_end].to_vec();
        Ok(Self { data, offsets })
    }
}
