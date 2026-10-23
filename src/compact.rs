pub mod leveled;
pub mod simple_leveled;
pub mod tiered;

use std::{
    collections::HashSet,
    fs,
    sync::Arc,
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    compact::{
        leveled::{LeveledCompactionController, LeveledCompactionOptions, LeveledCompactionTask},
        simple_leveled::{
            SimpleLeveledCompactionController, SimpleLeveledCompactionOptions,
            SimpleLeveledCompactionTask,
        },
        tiered::{TieredCompactionController, TieredCompactionOptions, TieredCompactionTask},
    },
    iterators::{
        StorageIterator, merge_iterator::MergeIterator, two_merge_iterators::TwoMergeIterator,
    },
    key::KeySlice,
    lsm_storage::{LsmStorageInner, LsmStorageState},
    table::{SsTable, builder::SsTableBuilder, iterator::SsTableIterator},
};

#[derive(Debug, Serialize, Deserialize)]
pub enum CompactionTask {
    Leveled(LeveledCompactionTask),
    Tiered(TieredCompactionTask),
    Simple(SimpleLeveledCompactionTask),
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
            CompactionTask::Simple(task) => task.is_lower_level_bottom_level,
            CompactionTask::Tiered(task) => task.bottom_tier_included,
        }
    }
}

pub(crate) enum CompactionController {
    Leveled(LeveledCompactionController),
    Tiered(TieredCompactionController),
    Simple(SimpleLeveledCompactionController),
    NoCompaction,
}

impl CompactionController {
    pub fn generate_compaction_task(&self, snapshot: &LsmStorageState) -> Option<CompactionTask> {
        use CompactionController as CC;
        use CompactionTask as CT;
        match self {
            CC::Leveled(ctrl) => ctrl.generate_compaction_task(snapshot).map(CT::Leveled),
            CC::Simple(ctrl) => ctrl.generate_compaction_task(snapshot).map(CT::Simple),
            CC::Tiered(ctrl) => ctrl.generate_compaction_task(snapshot).map(CT::Tiered),
            CC::NoCompaction => unreachable!(),
        }
    }

    pub fn apply_compaction_result(
        &self,
        snapshot: &LsmStorageState,
        task: &CompactionTask,
        output: &[usize],
        in_recovery: bool,
    ) -> (LsmStorageState, Vec<usize>) {
        use CompactionController as CC;
        use CompactionTask as CT;
        match (self, task) {
            (CC::Leveled(ctrl), CT::Leveled(task)) => {
                ctrl.apply_compaction_result(snapshot, task, output, in_recovery)
            }
            (CC::Simple(ctrl), CT::Simple(task)) => {
                ctrl.apply_compaction_result(snapshot, task, output)
            }
            (CC::Tiered(ctrl), CT::Tiered(task)) => {
                ctrl.apply_compaction_result(snapshot, task, output)
            }
            _ => unreachable!(),
        }
    }
}

impl CompactionController {
    pub fn flush_to_l0(&self) -> bool {
        matches!(
            self,
            Self::Leveled(_) | Self::Simple(_) | Self::NoCompaction
        )
    }
}

#[derive(Debug, Clone)]
pub enum CompactionOptions {
    /// Leveled compaction with partial compaction + dynamic level support (= RocksDB's Leveled
    /// Compaction)
    Leveled(LeveledCompactionOptions),

    /// Tiered compaction (= RocksDB's universal compaction)
    Tiered(TieredCompactionOptions),

    /// Simple leveled compaction
    Simple(SimpleLeveledCompactionOptions),

    /// In no compaction mode (week 1), always flush to L0
    NoCompaction,
}

impl LsmStorageInner {
    pub fn force_full_compaction(&self) -> Result<()> {
        let snapshot = {
            let state_guard = self.state.read();
            Arc::clone(&state_guard)
        };
        let l0_ss_tables = snapshot.l0_ss_tables.clone();
        let l1_ss_tables = snapshot.levels[0].1.clone();
        let task = CompactionTask::ForceFullCompaction {
            l0_ss_tables: l0_ss_tables.clone(),
            l1_ss_tables: l1_ss_tables.clone(),
        };
        let new_ssts = self.compact(&task)?;
        let mut new_sst_ids = Vec::with_capacity(new_ssts.len());

        let _state_lock = self.state_lock.lock();
        let mut state = self.state.read().as_ref().clone();

        for old_id in l0_ss_tables.iter().chain(l1_ss_tables.iter()) {
            let removed = state.ss_tables.remove(old_id);
            assert!(
                removed.is_some(),
                "compacted SST {old_id} missing from state"
            )
        }
        for new_sst in new_ssts {
            let sst_id = new_sst.sst_id();
            new_sst_ids.push(sst_id);
            let prev = state.ss_tables.insert(sst_id, new_sst);
            assert!(prev.is_none(), "duplicate SST ID {sst_id}");
        }
        assert_eq!(
            state.levels[0].1, l1_ss_tables,
            "L1 SSTs mutated concurrently during full compaction"
        );
        state.levels[0].1 = new_sst_ids;
        let mut captured: HashSet<usize> = l0_ss_tables.iter().copied().collect();
        state.l0_ss_tables.retain(|id| !captured.remove(id));
        assert!(
            captured.is_empty(),
            "some L0 SSTs in compaction task were missing from l0_ss_tables"
        );
        *self.state.write() = Arc::new(state);
        drop(_state_lock);
        for old_id in l0_ss_tables.iter().chain(l1_ss_tables.iter()) {
            fs::remove_file(self.path_of_sst(*old_id))?;
        }
        Ok(())
    }

    fn trigger_compaction(&self) -> Result<()> {
        unimplemented!()
    }

    /// Executes a `CompactionTask` and returns the newly generated `SsTable`s without modifying state.
    fn compact(&self, task: &CompactionTask) -> Result<Vec<Arc<SsTable>>> {
        let snapshot = {
            let state_guard = self.state.read();
            Arc::clone(&state_guard)
        };
        if let CompactionTask::ForceFullCompaction {
            l0_ss_tables,
            l1_ss_tables,
        } = task
        {
            let mut l0_iters = Vec::with_capacity(l0_ss_tables.len());
            for sst_idx in l0_ss_tables {
                let sst = snapshot
                    .ss_tables
                    .get(sst_idx)
                    .cloned()
                    .with_context(|| format!("missing L0 SST {sst_idx}"))?;
                l0_iters.push(Box::new(SsTableIterator::create_and_seek_to_first(sst)?));
            }
            let mut l1_iters = Vec::new();
            for sst_idx in l1_ss_tables {
                let sst = snapshot
                    .ss_tables
                    .get(sst_idx)
                    .cloned()
                    .with_context(|| format!("missing L1 SST {sst_idx}"))?;
                l1_iters.push(Box::new(SsTableIterator::create_and_seek_to_first(sst)?));
            }
            let two_merge_iter = TwoMergeIterator::create(
                MergeIterator::create(l0_iters),
                MergeIterator::create(l1_iters),
            )?;
            return self
                .compact_generate_sst_from_iter(two_merge_iter, task.compact_to_bottom_level());
        }
        unimplemented!("not implemented yet")
    }

    /// Drains a sorted iterator and builds targeted size SsTables on disk.
    fn compact_generate_sst_from_iter<I>(
        &self,
        mut iter: I,
        compact_to_bottom: bool,
    ) -> Result<Vec<Arc<SsTable>>>
    where
        I: 'static + for<'a> StorageIterator<KeyType<'a> = KeySlice<'a>>,
    {
        let mut new_ssts = Vec::new();
        let mut builder = None;

        while iter.is_valid() {
            if compact_to_bottom && iter.value().is_empty() {
                iter.next()?;
                continue;
            }
            let inner_builder =
                builder.get_or_insert_with(|| SsTableBuilder::new(self.options.block_size));
            inner_builder.add(iter.key(), iter.value());

            if inner_builder.estimated_size() >= self.options.target_sst_size {
                let inner_builder = builder.take().expect("None should've been caught");
                let sst_id = self.next_sst_id();
                new_ssts.push(Arc::new(inner_builder.build(
                    self.path_of_sst(sst_id).as_path(),
                    sst_id,
                    Some(self.block_cache.clone()),
                )?))
            }
            iter.next()?;
        }
        if let Some(inner_builder) = builder {
            let sst_id = self.next_sst_id();
            new_ssts.push(Arc::new(inner_builder.build(
                self.path_of_sst(sst_id),
                sst_id,
                Some(self.block_cache.clone()),
            )?));
        }
        Ok(new_ssts)
    }

    /// Spawns a background thread triggering a compaction every 50 milliseconds or returns
    /// early if receiver is closed.
    pub(crate) fn spawn_compaction_thread(
        self: &Arc<Self>,
        rx: crossbeam_channel::Receiver<()>,
    ) -> Result<Option<JoinHandle<()>>> {
        use CompactionOptions::*;
        if let NoCompaction = self.options.compaction_options {
            return Ok(None);
        }
        let inner = Arc::clone(self);
        let handle = thread::spawn(move || {
            let ticker = crossbeam_channel::tick(Duration::from_millis(50));
            loop {
                crossbeam_channel::select! {
                    recv(ticker) -> _ => if let Err(err) = inner.trigger_compaction() {
                        eprintln!("compaction failed: {:?}", err)
                    },
                    recv(rx) -> _ => return
                }
            }
        });
        Ok(Some(handle))
    }

    /// Evaluates the condition of flushing memtables which if true flushes the oldest
    /// memtable to the `l0_ss_table`.
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

    /// Spawns a background thread triggering a flush every 50 milliseconds or returns
    /// early if receiver is closed.
    pub(crate) fn spawn_flush_thread(
        self: &Arc<Self>,
        rx: crossbeam_channel::Receiver<()>,
    ) -> Result<Option<JoinHandle<()>>> {
        let inner = Arc::clone(self);

        let handle = thread::spawn(move || {
            let ticker = crossbeam_channel::tick(Duration::from_millis(50));
            loop {
                crossbeam_channel::select! {
                    recv(ticker) -> _ => if let Err(err) = inner.trigger_flush() {
                        eprintln!("flush failed: {}", err);
                    },
                    recv(rx) -> _ => return
                }
            }
        });
        Ok(Some(handle))
    }
}
