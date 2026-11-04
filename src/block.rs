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

mod builder;
mod iterator;

pub use builder::BlockBuilder;
use bytes::{Buf, BufMut, Bytes};
pub use iterator::BlockIterator;

use crate::block::builder::SIZEOF_U16;

/// A block is the smallest unit of read and caching in LSM tree. It is a collection of sorted key-value pairs.
pub struct Block {
    pub(crate) data: Vec<u8>,
    pub(crate) offsets: Vec<u16>,
}

impl Block {
    /// Encode the internal data to the data layout illustrated in the course
    /// Note: You may want to recheck if any of the expected field is missing from your output
    pub fn encode(&self) -> Bytes {
        let mut buffer = self.data.clone();
        for offset in &self.offsets {
            buffer.put_u16(*offset);
        }
        buffer.put_u16(u16::try_from(self.offsets.len()).expect("invalid offsets len"));
        buffer.into()
    }

    /// Decode from the data layout, transform the input `data` to a single `Block`
    pub fn decode(data: &[u8]) -> Self {
        let entry_count = usize::from((&data[data.len() - SIZEOF_U16..]).get_u16());
        let data_end = data.len() - SIZEOF_U16 - entry_count * SIZEOF_U16;
        let offsets = data[data_end..data.len() - SIZEOF_U16]
            .chunks(SIZEOF_U16)
            .map(|mut x| x.get_u16())
            .collect::<Vec<_>>();
        let data = &data[..data_end];
        Self {
            data: data.to_vec(),
            offsets,
        }
    }
}
