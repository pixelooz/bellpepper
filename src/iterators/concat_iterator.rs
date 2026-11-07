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

use std::sync::Arc;

use anyhow::Result;

use super::StorageIterator;
use crate::{
    key::KeySlice,
    table::{SsTable, SsTableIterator},
};

/// Concat multiple iterators ordered in key order and their key ranges do not overlap. We do not want to create the
/// iterators when initializing this iterator to reduce the overhead of seeking.
#[derive(Default)]
pub struct SstConcatIterator {
    current: Option<SsTableIterator>,
    next_sst_idx: usize,
    ss_tables: Vec<Arc<SsTable>>,
}

impl SstConcatIterator {
    pub fn create_and_seek_to_first(ss_tables: Vec<Arc<SsTable>>) -> Result<Self> {
        Self::check_sst_valid(&ss_tables);
        if ss_tables.is_empty() {
            return Ok(Self {
                ss_tables,
                ..Default::default()
            });
        }
        let sst_iter = SsTableIterator::create_and_seek_to_first(ss_tables[0].clone())?;
        let mut iter = Self {
            current: Some(sst_iter),
            next_sst_idx: 1,
            ss_tables,
        };
        iter.move_until_valid()?;
        Ok(iter)
    }

    pub fn create_and_seek_to_key(ss_tables: Vec<Arc<SsTable>>, key: KeySlice) -> Result<Self> {
        Self::check_sst_valid(&ss_tables);
        let index = ss_tables
            .partition_point(|sst| sst.first_key().as_key_slice() <= key)
            .saturating_sub(1);
        if index >= ss_tables.len() {
            return Ok(Self {
                ss_tables,
                ..Default::default()
            });
        }
        let sst_iter = SsTableIterator::create_and_seek_to_key(ss_tables[index].clone(), key)?;
        let mut iter = Self {
            current: Some(sst_iter),
            next_sst_idx: index + 1,
            ss_tables,
        };
        iter.move_until_valid()?;
        Ok(iter)
    }

    fn check_sst_valid(ss_tables: &[Arc<SsTable>]) {
        ss_tables
            .iter()
            .for_each(|sst| assert!(sst.first_key() <= sst.last_key()));
        ss_tables
            .windows(2)
            .for_each(|window| assert!(window[0].last_key() < window[1].first_key()));
    }

    fn move_until_valid(&mut self) -> Result<()> {
        while let Some(iter) = &self.current {
            if iter.is_valid() {
                break;
            }
            if self.next_sst_idx >= self.ss_tables.len() {
                self.current = None;
                continue;
            }
            let sst = self.ss_tables[self.next_sst_idx].clone();
            let curr_iter = SsTableIterator::create_and_seek_to_first(sst)?;
            self.current = Some(curr_iter);
            self.next_sst_idx += 1;
        }
        Ok(())
    }
}

impl StorageIterator for SstConcatIterator {
    type KeyType<'a> = KeySlice<'a>;

    fn key(&self) -> KeySlice<'_> {
        self.current.as_ref().unwrap().key()
    }

    fn value(&self) -> &[u8] {
        self.current.as_ref().unwrap().value()
    }

    fn is_valid(&self) -> bool {
        if let Some(iter) = &self.current {
            assert!(iter.is_valid());
            return true;
        }
        false
    }

    fn next(&mut self) -> Result<()> {
        self.current.as_mut().unwrap().next()?;
        self.move_until_valid()
    }

    fn num_active_iterators(&self) -> usize {
        1
    }
}
