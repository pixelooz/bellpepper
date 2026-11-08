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

mod leveled;

use std::collections::HashSet;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::Result;
pub use leveled::{LeveledCc, LeveledCompOpt, LeveledCompactionTask};
use serde::{Deserialize, Serialize};

use crate::iterators::StorageIterator;
use crate::iterators::concat_iterator::SstConcatIterator;
use crate::iterators::merge_iterator::MergeIterator;
use crate::iterators::two_merge_iterator::TwoMergeIterator;
use crate::key::KeySlice;
use crate::lsm_storage::{LsmStorageInner, LsmStorageState};
use crate::table::{SsTable, SsTableBuilder, SsTableIterator};

#[derive(Debug, Serialize, Deserialize)]
pub enum CompactionTask {
    Leveled(LeveledCompactionTask),
    ForceFullCompaction {
        l0_ss_tables: Vec<usize>,
        l1_ss_tables: Vec<usize>,
    },
}

impl CompactionTask {
    fn compact_to_bottom_level(&self) -> bool {
        match self {
            CompactionTask::ForceFullCompaction { .. } => true,
            CompactionTask::Leveled(task) => task.is_lower_level_bottom_level,
        }
    }
}

pub(crate) enum CompactionController {
    Leveled(LeveledCc),
    NoCompaction,
}

impl CompactionController {
    pub fn generate_compaction_task(&self, snapshot: &LsmStorageState) -> Option<CompactionTask> {
        use CompactionController as Cc;
        match self {
            Cc::Leveled(ctrl) => ctrl
                .generate_compaction_task(snapshot)
                .map(CompactionTask::Leveled),
            Cc::NoCompaction => unreachable!(),
        }
    }

    pub fn apply_compaction_result(
        &self,
        snapshot: &LsmStorageState,
        task: &CompactionTask,
        output: &[usize],
        in_recovery: bool,
    ) -> (LsmStorageState, Vec<usize>) {
        use CompactionController as Cc;
        match (self, task) {
            (Cc::Leveled(ctrl), CompactionTask::Leveled(task)) => {
                ctrl.apply_compaction_result(snapshot, task, output, in_recovery)
            }
            _ => unreachable!(),
        }
    }

    pub fn flush_to_l0(&self) -> bool {
        matches!(self, Self::Leveled(_) | Self::NoCompaction)
    }
}

#[derive(Debug, Clone)]
pub enum CompactionOptions {
    /// Leveled compaction with partial compaction + dynamic level support (= RocksDB's Leveled
    /// Compaction)
    Leveled(LeveledCompOpt),
    /// In no compaction mode (week 1), always flush to L0
    NoCompaction,
}

impl LsmStorageInner {
    fn compact(&self, task: &CompactionTask) -> Result<Vec<Arc<SsTable>>> {
        match task {
            CompactionTask::ForceFullCompaction {
                l0_ss_tables,
                l1_ss_tables,
            } => {
                let snapshot = {
                    let state = self.state.read();
                    Arc::clone(&state)
                };
                let mut l0_iters = Vec::with_capacity(l0_ss_tables.len());
                for sst_id in l0_ss_tables {
                    let sst = Arc::clone(&snapshot.ss_tables[sst_id]);
                    let iter = SsTableIterator::create_and_seek_to_first(sst)?;
                    l0_iters.push(Box::new(iter));
                }
                let mut l1_iters = Vec::with_capacity(l1_ss_tables.len());
                for sst_id in l1_ss_tables {
                    let sst = Arc::clone(&snapshot.ss_tables[sst_id]);
                    l1_iters.push(Arc::clone(&sst));
                }
                let iter = TwoMergeIterator::create(
                    MergeIterator::create(l0_iters),
                    SstConcatIterator::create_and_seek_to_first(l1_iters)?,
                )?;
                self.generate_sst_from_iter(iter, true)
            }
            CompactionTask::Leveled(LeveledCompactionTask {
                upper_level,
                upper_level_sst_ids,
                lower_level,
                lower_level_sst_ids,
                is_lower_level_bottom_level,
            }) => {
                let snapshot = {
                    let state = self.state.read();
                    Arc::clone(&state)
                };
                let mut lower_ssts = Vec::with_capacity(lower_level_sst_ids.len());
                for sst_id in lower_level_sst_ids {
                    let sst = Arc::clone(&snapshot.ss_tables[sst_id]);
                    lower_ssts.push(sst);
                }
                let lower_iter = SstConcatIterator::create_and_seek_to_first(lower_ssts)?;
                if upper_level.is_some() {
                    let mut upper_ssts = Vec::with_capacity(upper_level_sst_ids.len());
                    for sst_id in upper_level_sst_ids {
                        let sst = Arc::clone(&snapshot.ss_tables[sst_id]);
                        upper_ssts.push(sst);
                    }
                    let upper_iter = SstConcatIterator::create_and_seek_to_first(upper_ssts)?;
                    let iter = TwoMergeIterator::create(upper_iter, lower_iter)?;
                    self.generate_sst_from_iter(iter, *is_lower_level_bottom_level)
                } else {
                    let mut upper_ssts = Vec::with_capacity(snapshot.l0_ss_tables.len());
                    for sst_id in &snapshot.l0_ss_tables {
                        let sst = Arc::clone(&snapshot.ss_tables[sst_id]);
                        let iter = SsTableIterator::create_and_seek_to_first(sst)?;
                        upper_ssts.push(Box::new(iter));
                    }
                    let upper_iter = MergeIterator::create(upper_ssts);
                    let iter = TwoMergeIterator::create(upper_iter, lower_iter)?;
                    self.generate_sst_from_iter(iter, *is_lower_level_bottom_level)
                }
            }
        }
    }

    fn generate_sst_from_iter<I>(
        &self,
        mut iter: I,
        compact_to_bottom: bool,
    ) -> Result<Vec<Arc<SsTable>>>
    where
        I: for<'a> StorageIterator<KeyType<'a> = KeySlice<'a>>,
    {
        let mut builder_opt: Option<SsTableBuilder> = None;
        let mut ss_tables = Vec::new();
        while iter.is_valid() {
            if compact_to_bottom && iter.value().is_empty() {
                iter.next()?;
                continue;
            }
            if builder_opt.is_none() {
                builder_opt = Some(SsTableBuilder::new(self.options.block_size));
            }
            let builder = builder_opt.as_mut().unwrap();
            builder.add(iter.key(), iter.value());
            if builder.estimated_size() >= self.options.intended_sst_size {
                let sst_id = self.next_sst_id();
                let builder = builder_opt.take().unwrap();
                let sst = builder.build(
                    sst_id,
                    Some(Arc::clone(&self.block_cache)),
                    self.path_of_sst(sst_id),
                )?;
                ss_tables.push(Arc::new(sst));
            }
            iter.next()?;
        }
        if let Some(builder) = builder_opt {
            let sst_id = self.next_sst_id();
            let sst = builder.build(
                sst_id,
                Some(Arc::clone(&self.block_cache)),
                self.path_of_sst(sst_id),
            )?;
            ss_tables.push(Arc::new(sst));
        }
        Ok(ss_tables)
    }

    pub fn force_full_compaction(&self) -> Result<()> {
        let CompactionOptions::NoCompaction = self.options.compaction_options else {
            panic!(
                "compaction mode other than `NoCompaction`: {:?}",
                self.options.compaction_options
            )
        };
        let snapshot = {
            let state = self.state.read();
            Arc::clone(&state)
        };
        let l0_ss_tables = &snapshot.l0_ss_tables;
        let l1_ss_tables = &snapshot.levels[0].1;
        let task = CompactionTask::ForceFullCompaction {
            l0_ss_tables: l0_ss_tables.clone(),
            l1_ss_tables: l1_ss_tables.clone(),
        };
        let new_ssts = self.compact(&task)?;
        let _state_guard = self.state_lock.lock();
        let mut snapshot = self.state.read().as_ref().clone();
        for sst_id in l0_ss_tables.iter().chain(l1_ss_tables.iter()) {
            let removed = snapshot.ss_tables.remove(sst_id);
            assert!(removed.is_some())
        }
        let mut new_ids = Vec::with_capacity(new_ssts.len());
        for new_sst in new_ssts {
            let sst_id = new_sst.sst_id();
            new_ids.push(sst_id);
            let prev = snapshot.ss_tables.insert(sst_id, new_sst);
            assert!(prev.is_none(), "duplicate sst_id");
        }
        assert_eq!(l1_ss_tables, &snapshot.levels[0].1,);
        snapshot.levels[0].1.clone_from(&new_ids);
        let mut l0_sst_set = snapshot
            .l0_ss_tables
            .iter()
            .copied()
            .collect::<HashSet<usize>>();
        snapshot.l0_ss_tables.retain(|id| !l0_sst_set.remove(id));
        assert!(l0_sst_set.is_empty());
        *self.state.write() = Arc::new(snapshot);
        drop(_state_guard);
        for sst_id in l0_ss_tables.iter().chain(l1_ss_tables) {
            std::fs::remove_file(self.path_of_sst(*sst_id))?;
        }
        Ok(())
    }

    fn trigger_compaction(&self) -> Result<()> {
        unimplemented!()
    }

    pub(crate) fn spawn_compaction_thread(
        self: &Arc<Self>,
        rx: crossbeam_channel::Receiver<()>,
    ) -> Result<Option<JoinHandle<()>>> {
        if let CompactionOptions::NoCompaction = self.options.compaction_options {
            return Ok(None);
        }
        let this = self.clone();
        let handle = thread::spawn(move || {
            let ticker = crossbeam_channel::tick(Duration::from_millis(50));
            loop {
                crossbeam_channel::select! {
                    recv(ticker) -> _ => if let Err(e) = this.trigger_compaction() {
                        eprintln!("compaction failed: {}", e);
                    },
                    recv(rx) -> _ => return
                }
            }
        });
        Ok(Some(handle))
    }

    fn trigger_flush(&self) -> Result<()> {
        let should_flush = {
            let state_guard = self.state.read();
            state_guard.imm_memtables.len() >= self.options.num_memtable_limit
        };
        if should_flush {
            self.force_flush_next_imm_memtable()?;
        }
        Ok(())
    }

    pub(crate) fn spawn_flush_thread(
        self: &Arc<Self>,
        rx: crossbeam_channel::Receiver<()>,
    ) -> Result<Option<JoinHandle<()>>> {
        let this = self.clone();
        let handle = thread::spawn(move || {
            let ticker = crossbeam_channel::tick(Duration::from_millis(50));
            loop {
                crossbeam_channel::select! {
                    recv(ticker) -> _ => if let Err(e) = this.trigger_flush() {
                        eprintln!("flush failed: {}", e);
                    },
                    recv(rx) -> _ => return
                }
            }
        });
        Ok(Some(handle))
    }
}
