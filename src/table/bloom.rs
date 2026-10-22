use std::{cmp, f64};

use anyhow::Result;
use bytes::{BufMut, Bytes, BytesMut};

/// Implements a bloom filter.
pub struct Bloom {
    /// Data of filter in bits.
    pub(crate) filter: Bytes,

    /// Number of hash functions.
    pub(crate) k: u8,
}

pub trait BitSlice {
    fn get_bit(&self, bit_num: usize) -> bool;
    fn bit_len(&self) -> usize;
}

pub trait BitSliceMut {
    fn set_bit(&mut self, bit_num: usize, value: bool);
}

impl<T: AsRef<[u8]>> BitSlice for T {
    fn get_bit(&self, bit_num: usize) -> bool {
        let index = bit_num / 8;
        let offset = bit_num % 8;
        self.as_ref()[index] & (1 << offset) != 0
    }

    fn bit_len(&self) -> usize {
        self.as_ref().len()
    }
}

impl<T: AsMut<[u8]>> BitSliceMut for T {
    fn set_bit(&mut self, bit_num: usize, value: bool) {
        let index = bit_num / 8;
        let offset = bit_num % 8;
        if value {
            self.as_mut()[index] |= 1 << offset;
        } else {
            self.as_mut()[index] &= !(1 << offset);
        }
    }
}

impl Bloom {
    /// Encode a bloom filter.
    pub fn encode(&self, buffer: &mut Vec<u8>) {
        buffer.extend(&self.filter);
        buffer.put_u8(self.k);
    }

    /// Decode a bloom filter.
    pub fn decode(data: &[u8]) -> Result<Self> {
        let filter = &data[..data.len() - 1];
        let k = data[data.len() - 1];
        Ok(Self {
            filter: filter.to_vec().into(),
            k,
        })
    }

    /// Get bloom filter bits per key from entry count and false positive rates.
    pub fn bloom_bits_per_key(entries: usize, false_positive_rates: f64) -> usize {
        let size = -(entries as f64) * false_positive_rates.ln() / f64::consts::LN_2.powi(2);
        let bits_per_key = (size / (entries as f64)).ceil();
        bits_per_key as usize
    }

    /// Build bloom filter from key hashes.
    pub fn build_from_key_hashes(keys: &[u32], bits_per_key: usize) -> Self {
        let k = (bits_per_key as f64 * 0.69) as u32;
        let k = k.clamp(1, 30);
        let n_bits = cmp::max(keys.len() * bits_per_key, 64);
        let n_bytes = n_bits.div_ceil(8);
        let n_bits = n_bytes * 8;
        let mut filter = BytesMut::with_capacity(n_bytes);
        filter.resize(n_bytes, 0);
        for hash in keys {
            let mut h = *hash;
            let delta = h.rotate_left(15);
            for _ in 0..k {
                let bit_idx = (h as usize) % n_bits;
                filter.set_bit(bit_idx, true);
                h = h.wrapping_add(delta);
            }
        }
        Self {
            filter: filter.freeze(),
            k: k as u8,
        }
    }

    /// Check if the bloom filter may contain some data.
    pub fn may_contain(&self, mut h: u32) -> bool {
        if self.k <= 30 {
            let n_bits = self.filter.bit_len();
            let delta = h.rotate_left(15);
            for _ in 0..self.k {
                let bit_idx = (h as usize) % n_bits;
                if !self.filter.get_bit(bit_idx) {
                    return false;
                }
                h = h.wrapping_add(delta);
            }
        }
        true
    }
}
