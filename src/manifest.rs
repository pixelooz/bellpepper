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

#![allow(unused_variables)]
#![allow(dead_code)]

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, bail};
use bytes::BufMut;
use parking_lot::{Mutex, MutexGuard};
use serde::{Deserialize, Serialize};

use crate::compact::CompactionTask;

pub struct Manifest {
    file: Arc<Mutex<File>>,
}

#[derive(Serialize, Deserialize)]
pub enum ManifestRecord {
    Flush(usize),
    NewMemtable(usize),
    Compaction(CompactionTask, Vec<usize>),
}

impl Manifest {
    pub fn create(path: impl AsRef<Path>) -> Result<Self> {
        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path)?;
        Ok(Self {
            file: Arc::new(Mutex::new(file)),
        })
    }

    pub fn recover(path: impl AsRef<Path>) -> Result<(Self, Vec<ManifestRecord>)> {
        let mut file = OpenOptions::new().read(true).append(true).open(path)?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;
        let mut records = Vec::new();
        let mut valid_len = 0usize;
        while valid_len < buffer.len() {
            let remaining = &buffer[valid_len..];
            if remaining.len() < 8 {
                break;
            }
            let len = u64::from_be_bytes(remaining[0..8].try_into().unwrap()) as usize;
            let Some(block_len) = 8usize.checked_add(len).and_then(|n| n.checked_add(4)) else {
                break;
            };
            if remaining.len() < block_len {
                break;
            }
            let body = &remaining[8..8 + len];
            let hash = u32::from_be_bytes(remaining[8 + len..8 + len + 4].try_into().unwrap());
            if hash != crc32fast::hash(body) {
                bail!("manifest checksum mismatched");
            }
            let record = serde_json::from_slice::<ManifestRecord>(body)?;
            records.push(record);
            valid_len += block_len;
        }
        if valid_len < buffer.len() {
            file.set_len(u64::try_from(valid_len).unwrap())?;
            file.sync_all()?;
        }
        Ok((
            Self {
                file: Arc::new(Mutex::new(file)),
            },
            records,
        ))
    }

    pub fn add_record(
        &self,
        _state_lock_observer: &MutexGuard<()>,
        record: ManifestRecord,
    ) -> Result<()> {
        self.add_record_when_init(record)
    }

    pub fn add_record_when_init(&self, record: ManifestRecord) -> Result<()> {
        let mut file = self.file.lock();
        let mut buffer = serde_json::to_vec(&record)?;
        let hash = crc32fast::hash(&buffer);
        file.write_all(&(u64::try_from(buffer.len()).unwrap()).to_be_bytes())?;
        buffer.put_u32(hash);
        file.write_all(&buffer)?;
        file.sync_all()?;
        Ok(())
    }
}
