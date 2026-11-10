// REMOVE THIS LINE after fully implementing this functionality
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

use anyhow::{Result, ensure};
use bytes::{Buf, BufMut, Bytes};
use crossbeam_skiplist::SkipMap;
use parking_lot::Mutex;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::Path;
use std::sync::Arc;

use crate::key::KeySlice;

pub struct Wal {
    file: Arc<Mutex<BufWriter<File>>>,
}

impl Wal {
    /// Week 2 Day 6: create a new write-ahead log.
    pub fn create(path: impl AsRef<Path>) -> Result<Self> {
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .read(true)
            .open(path)?;
        Ok(Self {
            file: Arc::new(Mutex::new(BufWriter::new(file))),
        })
    }

    /// Week 2 Day 6: recover entries from a write-ahead log.
    pub fn recover(path: impl AsRef<Path>, skiplist: &SkipMap<Bytes, Bytes>) -> Result<Self> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .read(true)
            .open(path)?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;
        let mut rbuf = buffer.as_slice();
        while rbuf.has_remaining() {
            let len_before = rbuf.len();
            let key_len = usize::from(rbuf.get_u16());
            let key = Bytes::copy_from_slice(&rbuf[..key_len]);
            rbuf.advance(key_len);
            let value_len = usize::from(rbuf.get_u16());
            let value = Bytes::copy_from_slice(&rbuf[..value_len]);
            rbuf.advance(value_len);
            let payload = len_before - rbuf.len();
            let checksum = rbuf.get_u32();
            ensure!(
                crc32fast::hash(&rbuf[..payload]) == checksum,
                "Wal checksum mismatch"
            );
            skiplist.insert(key, value);
        }
        Ok(Self {
            file: Arc::new(Mutex::new(BufWriter::new(file))),
        })
    }

    /// Week 2 Day 6: append a key-value pair to the write-ahead log.
    pub fn put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        let mut file = self.file.lock();
        let mut buffer =
            Vec::with_capacity(2 * size_of::<u16>() + key.len() + value.len() + size_of::<u32>());
        buffer.put_u16(key.len().try_into().unwrap());
        buffer.put(key);
        buffer.put_u16(value.len().try_into().unwrap());
        buffer.put(value);
        let checksum = crc32fast::hash(&buffer);
        buffer.put_u32(checksum);
        file.write_all(&buffer)?;
        Ok(())
    }

    /// Week 3 Day 5: append a batch of key-value pairs.
    pub fn put_batch(&self, _data: &[(KeySlice, &[u8])]) -> Result<()> {
        unimplemented!()
    }

    /// Week 2 Day 6: synchronize the write-ahead log to storage.
    pub fn sync(&self) -> Result<()> {
        let mut file = self.file.lock();
        file.flush()?;
        file.get_mut().sync_all()?;
        Ok(())
    }
}
