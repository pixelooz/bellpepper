pub(crate) mod builder;
pub(crate) mod iterator;

use std::mem;

use bytes::{Buf, BufMut, Bytes};

pub(crate) const SIZEOF_U16: usize = mem::size_of::<u16>();

/// A block is the smallest unit of read and caching in LSM tree. It is a collection of
/// sorted key-value pairs.
#[derive(Debug, Clone)]
pub struct Block {
    pub(crate) data: Vec<u8>,
    pub(crate) offsets: Vec<u16>,
}

impl Block {
    /// Encode the internal data to the data layout illustrated in the course
    pub fn encode(&self) -> Bytes {
        let mut buffer = self.data.clone();
        let entry_count = self.offsets.len();
        for offset in &self.offsets {
            buffer.put_u16(*offset);
        }
        buffer.put_u16(entry_count as u16);
        buffer.into()
    }

    /// Decode from the data layout, transform the input `data` to a single `Block`
    pub fn decode(data: &[u8]) -> Self {
        let entry_count = usize::from((&data[data.len() - SIZEOF_U16..]).get_u16());
        assert!(entry_count > 0, "block has no entries");

        let data_end = data.len() - SIZEOF_U16 - entry_count * SIZEOF_U16;
        let offsets = data[data_end..data.len() - SIZEOF_U16]
            .chunks(SIZEOF_U16)
            .map(|mut offset| offset.get_u16())
            .collect::<Vec<u16>>();
        assert!(offsets.windows(2).all(|pair| pair[0] < pair[1]));
        Self {
            data: data[..data_end].to_vec(),
            offsets,
        }
    }
}
