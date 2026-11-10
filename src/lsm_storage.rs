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

use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::{cmp, mem};

use anyhow::{Result, anyhow, ensure};
use bytes::Bytes;
use parking_lot::{Mutex, MutexGuard, RwLock};

use crate::block::Block;
use crate::compact::{CompactionController, CompactionOptions, LeveledCc, LeveledCompOpt};
use crate::iterators::StorageIterator;
use crate::iterators::concat_iterator::SstConcatIterator;
use crate::iterators::merge_iterator::MergeIterator;
use crate::iterators::two_merge_iterator::TwoMergeIterator;
use crate::key::KeySlice;
use crate::lsm_iterator::{FusedIterator, LsmIterator};
use crate::manifest::{Manifest, ManifestRecord};
use crate::mem_table::{MemTable, map_bound};
use crate::mvcc::LsmMvccInner;
use crate::table::{FileObject, SsTable, SsTableBuilder, SsTableIterator};

pub type BlockCache = moka::sync::Cache<(usize, usize), Arc<Block>>;

/// Represents the state of the storage engine.
#[derive(Clone)]
pub struct LsmStorageState {
    /// The current memtable.
    pub memtable: Arc<MemTable>,
    /// Immutable memtables, from latest to earliest.
    pub imm_memtables: Vec<Arc<MemTable>>,
    /// L0 SSTs, from latest to earliest.
    pub l0_ss_tables: Vec<usize>,
    /// SsTables sorted by key range; L1 - L_max for leveled compaction, or tiers for tiered
    /// compaction.
    pub levels: Vec<(usize, Vec<usize>)>,
    /// SST objects.
    pub ss_tables: HashMap<usize, Arc<SsTable>>,
}

pub enum WriteBatchRecord<T: AsRef<[u8]>> {
    Put(T, T),
    Del(T),
}

impl LsmStorageState {
    fn create(options: &LsmStorageOptions) -> Self {
        use CompactionOptions as Co;
        let levels = match &options.compaction_options {
            Co::Leveled(LeveledCompOpt { max_levels, .. }) => (1..=*max_levels)
                .map(|level| (level, Vec::new()))
                .collect::<Vec<_>>(),
            Co::NoCompaction => vec![(1, Vec::new())],
        };
        Self {
            memtable: Arc::new(MemTable::create(0)),
            imm_memtables: Vec::new(),
            l0_ss_tables: Vec::new(),
            levels,
            ss_tables: Default::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LsmStorageOptions {
    // Block size in bytes
    pub block_size: usize,
    // SST size in bytes, also the approximate memtable capacity limit
    pub intended_sst_size: usize,
    // Maximum number of memtables in memory, flush to L0 when exceeding this limit
    pub num_memtable_limit: usize,
    pub compaction_options: CompactionOptions,
    pub enable_wal: bool,
    pub serializable: bool,
}

#[derive(Clone, Debug)]
pub enum CompactionFilter {
    Prefix(Bytes),
}

/// The storage interface of the LSM tree.
pub(crate) struct LsmStorageInner {
    pub(crate) state: Arc<RwLock<Arc<LsmStorageState>>>,
    pub(crate) state_lock: Mutex<()>,
    path: PathBuf,
    pub(crate) block_cache: Arc<BlockCache>,
    next_sst_id: AtomicUsize,
    pub(crate) options: Arc<LsmStorageOptions>,
    pub(crate) compaction_controller: CompactionController,
    pub(crate) manifest: Option<Manifest>,
    pub(crate) mvcc: Option<LsmMvccInner>,
    pub(crate) compaction_filters: Arc<Mutex<Vec<CompactionFilter>>>,
}

/// A thin wrapper for `LsmStorageInner` and the user interface for MiniLSM.
pub struct MiniLsm {
    pub(crate) inner: Arc<LsmStorageInner>,
    /// Notifies the L0 flush thread to stop working. (In week 1 day 6)
    flush_notifier: crossbeam_channel::Sender<()>,
    /// The handle for the flush thread. (In week 1 day 6)
    flush_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// Notifies the compaction thread to stop working. (In week 2)
    compaction_notifier: crossbeam_channel::Sender<()>,
    /// The handle for the compaction thread. (In week 2)
    compaction_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Drop for MiniLsm {
    fn drop(&mut self) {
        self.compaction_notifier.send(()).ok();
        self.flush_notifier.send(()).ok();
    }
}

impl MiniLsm {
    pub fn close(&self) -> Result<()> {
        self.compaction_notifier.send(()).ok();
        self.flush_notifier.send(()).ok();
        let mut flush_thread = self.flush_thread.lock();
        if let Some(flush_thread) = flush_thread.take() {
            flush_thread.join().map_err(|err| anyhow!("{:?}", err))?;
        }
        let mut compaction_thread = self.compaction_thread.lock();
        if let Some(compaction_thread) = compaction_thread.take() {
            compaction_thread
                .join()
                .map_err(|err| anyhow!("{:?}", err))?;
        }
        if !self.inner.state.read().memtable.is_empty() {
            self.inner
                .replace_memtable(Arc::new(MemTable::create(self.inner.next_sst_id())))?;
        }
        while {
            let state = self.inner.state.read();
            !state.imm_memtables.is_empty()
        } {
            self.inner.force_flush_next_imm_memtable()?;
        }
        self.inner.sync_dir()?;
        Ok(())
    }

    /// Start the storage engine by either loading an existing directory or creating a new one if the directory does
    /// not exist.
    pub fn open(path: impl AsRef<Path>, options: LsmStorageOptions) -> Result<Arc<Self>> {
        let inner = Arc::new(LsmStorageInner::open(path, options)?);
        let (tx1, rx) = crossbeam_channel::unbounded();
        let compaction_thread = inner.spawn_compaction_thread(rx)?;
        let (tx2, rx) = crossbeam_channel::unbounded();
        let flush_thread = inner.spawn_flush_thread(rx)?;
        Ok(Arc::new(Self {
            inner,
            flush_notifier: tx2,
            flush_thread: Mutex::new(flush_thread),
            compaction_notifier: tx1,
            compaction_thread: Mutex::new(compaction_thread),
        }))
    }

    /// Only call this in test cases due to race conditions
    pub fn force_flush(&self) -> Result<()> {
        if !self.inner.state.read().memtable.is_empty() {
            self.inner
                .force_freeze_memtable(&self.inner.state_lock.lock())?;
        }
        if !self.inner.state.read().imm_memtables.is_empty() {
            self.inner.force_flush_next_imm_memtable()?;
        }
        Ok(())
    }
}

impl LsmStorageInner {
    pub(crate) fn next_sst_id(&self) -> usize {
        self.next_sst_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    /// Start the storage engine by either loading an existing directory or creating a new one if the directory does
    /// not exist.
    pub(crate) fn open(path: impl AsRef<Path>, options: LsmStorageOptions) -> Result<Self> {
        let block_cache = Arc::new(BlockCache::new(1 << 20));
        let mut state = LsmStorageState::create(&options);
        let mut next_sst_id = 1;
        let path = path.as_ref();
        if !path.exists() {
            std::fs::create_dir_all(path)?;
        }
        let compaction_controller = match &options.compaction_options {
            CompactionOptions::Leveled(options) => {
                CompactionController::Leveled(LeveledCc::new(options.clone()))
            }
            CompactionOptions::NoCompaction => CompactionController::NoCompaction,
        };
        let manifest_path = path.join("MANIFEST");
        let manifest;
        if !manifest_path.exists() {
            manifest = Some(Manifest::create(manifest_path)?);
            manifest
                .as_ref()
                .unwrap()
                .add_record_when_init(ManifestRecord::NewMemtable(state.memtable.id()))?;
        } else {
            let (_manifest, records) = Manifest::recover(manifest_path)?;
            let mut memtables = BTreeSet::new();
            for record in records {
                match record {
                    ManifestRecord::Flush(sst_id) => {
                        let prev = memtables.remove(&sst_id);
                        assert!(prev, "memtable did not exist before flushing?");
                        state.l0_ss_tables.insert(0, sst_id);
                        next_sst_id = cmp::max(next_sst_id, sst_id);
                    }
                    ManifestRecord::NewMemtable(sst_id) => {
                        next_sst_id = cmp::max(next_sst_id, sst_id);
                        memtables.insert(sst_id);
                    }
                    ManifestRecord::Compaction(task, sst_ids) => {
                        let (new_state, _) = compaction_controller
                            .apply_compaction_result(&state, &task, &sst_ids, true);
                        state = new_state;
                        next_sst_id =
                            next_sst_id.max(sst_ids.iter().max().copied().unwrap_or_default());
                    }
                }
            }
            for &sst_id in state
                .l0_ss_tables
                .iter()
                .chain(state.levels.iter().flat_map(|(_, sst_ids)| sst_ids))
            {
                let file = FileObject::open(&Self::path_of_sst_static(path, sst_id))?;
                let ss_table = SsTable::open(sst_id, Some(Arc::clone(&block_cache)), file)?;
                state.ss_tables.insert(sst_id, Arc::new(ss_table));
            }
            if let CompactionController::Leveled(_) = compaction_controller {
                for (_, sst_ids) in &mut state.levels {
                    sst_ids.sort_by(|a, b| {
                        state.ss_tables[a]
                            .first_key()
                            .cmp(state.ss_tables[b].first_key())
                    });
                }
            }
            next_sst_id += 1;
            state.memtable = Arc::new(MemTable::create(next_sst_id));
            _manifest.add_record_when_init(ManifestRecord::NewMemtable(state.memtable.id()))?;
            next_sst_id += 1;
            manifest = Some(_manifest)
        }
        let storage = Self {
            state: Arc::new(RwLock::new(Arc::new(state))),
            state_lock: Mutex::new(()),
            path: path.to_path_buf(),
            block_cache,
            next_sst_id: AtomicUsize::new(next_sst_id),
            compaction_controller,
            manifest,
            options: options.into(),
            mvcc: None,
            compaction_filters: Arc::new(Mutex::new(Vec::new())),
        };
        Ok(storage)
    }

    pub fn sync(&self) -> Result<()> {
        unimplemented!()
    }

    pub fn add_compaction_filter(&self, compaction_filter: CompactionFilter) {
        let mut compaction_filters = self.compaction_filters.lock();
        compaction_filters.push(compaction_filter);
    }

    /// Get a key from the storage. In day 7, this can be further optimized by using a bloom filter.
    pub fn get(&self, key: &[u8]) -> Result<Option<Bytes>> {
        let snapshot = {
            let guard = self.state.read();
            Arc::clone(&guard)
        };
        if let Some(value) = snapshot.memtable.get(key) {
            if value.is_empty() {
                return Ok(None);
            }
            return Ok(Some(value));
        }
        for imm_memtable in &snapshot.imm_memtables {
            if let Some(value) = imm_memtable.get(key) {
                if value.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(value));
            }
        }
        let keep_table = |key: &[u8], table: &SsTable| {
            if table.first_key().raw_ref() <= key && key <= table.last_key().raw_ref() {
                if let Some(bloom) = &table.bloom {
                    return bloom.may_contain(farmhash::fingerprint32(key));
                }
                return true;
            }
            false
        };
        let mut l0_iters = Vec::with_capacity(snapshot.l0_ss_tables.len());
        for table_id in &snapshot.l0_ss_tables {
            let table = snapshot.ss_tables[table_id].clone();
            if !keep_table(key, &table) {
                continue;
            }
            let iter = SsTableIterator::create_and_seek_to_key(table, KeySlice::from_slice(key))?;
            l0_iters.push(Box::new(iter));
        }
        let mut concat_iters = Vec::with_capacity(snapshot.levels.len());
        for (_, level_sst_ids) in &snapshot.levels {
            let mut level_ssts = Vec::with_capacity(level_sst_ids.len());
            for sst_id in level_sst_ids {
                let sst = Arc::clone(&snapshot.ss_tables[sst_id]);
                if !keep_table(key, &sst) {
                    continue;
                }
                level_ssts.push(sst);
            }
            let concat_iter =
                SstConcatIterator::create_and_seek_to_key(level_ssts, KeySlice::from_slice(key))?;
            concat_iters.push(Box::new(concat_iter));
        }
        let iter = TwoMergeIterator::create(
            MergeIterator::create(l0_iters),
            MergeIterator::create(concat_iters),
        )?;
        if iter.is_valid() && iter.key().raw_ref() == key && !iter.value().is_empty() {
            return Ok(Some(Bytes::copy_from_slice(iter.value())));
        }
        Ok(None)
    }

    /// Write a batch of data into the storage. Implement in week 2 day 7.
    pub fn write_batch<T: AsRef<[u8]>>(&self, _batch: &[WriteBatchRecord<T>]) -> Result<()> {
        unimplemented!()
    }

    /// Put a key-value pair into the storage by writing into the current memtable.
    pub fn put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        ensure!(!key.is_empty(), "key cannot be empty");
        let estimated_size = {
            let guard = self.state.read();
            guard.memtable.put(key, value)?;
            guard.memtable.approximate_size()
        };
        self.try_freeze(estimated_size)?;
        Ok(())
    }

    /// Remove a key from the storage by writing an empty value.
    pub fn delete(&self, key: &[u8]) -> Result<()> {
        self.put(key, b"")
    }

    pub(super) fn sync_dir(&self) -> Result<()> {
        File::open(&self.path)?.sync_all()?;
        Ok(())
    }

    fn try_freeze(&self, estimated_size: usize) -> Result<()> {
        if estimated_size >= self.options.intended_sst_size {
            let state_lock_guard = self.state_lock.lock();
            let state_guard = self.state.read();
            if state_guard.memtable.approximate_size() >= self.options.intended_sst_size {
                drop(state_guard);
                self.force_freeze_memtable(&state_lock_guard)?;
            }
        }
        Ok(())
    }

    /// Force freeze the current memtable to an immutable memtable.
    pub fn force_freeze_memtable(&self, _state_lock_observer: &MutexGuard<'_, ()>) -> Result<()> {
        let next_sst_id = self.next_sst_id();
        let new_memtable = Arc::new(MemTable::create(next_sst_id));
        self.replace_memtable(new_memtable)?;
        self.manifest.as_ref().unwrap().add_record(
            _state_lock_observer,
            ManifestRecord::NewMemtable(next_sst_id),
        )?;
        Ok(())
    }

    fn replace_memtable(&self, new_memtable: Arc<MemTable>) -> Result<()> {
        let mut state_guard = self.state.write();
        let mut snapshot = state_guard.as_ref().clone();
        let old_memtable = mem::replace(&mut snapshot.memtable, new_memtable);
        snapshot.imm_memtables.insert(0, old_memtable);
        *state_guard = Arc::new(snapshot);
        drop(state_guard);
        Ok(())
    }

    /// Force flush the earliest-created immutable memtable to disk.
    pub fn force_flush_next_imm_memtable(&self) -> Result<()> {
        let _state_lock = self.state_lock.lock();
        let flush_memtable = {
            let guard = self.state.read();
            if let Some(last_memtable) = guard.imm_memtables.last() {
                Arc::clone(last_memtable)
            } else {
                return Ok(());
            }
        };
        let mut builder = SsTableBuilder::new(self.options.block_size);
        flush_memtable.flush(&mut builder)?;
        let memtable_id = flush_memtable.id();
        let sst = Arc::new(builder.build(
            memtable_id,
            Some(self.block_cache.clone()),
            self.path_of_sst(memtable_id),
        )?);
        self.manifest
            .as_ref()
            .unwrap()
            .add_record(&_state_lock, ManifestRecord::Flush(memtable_id))?;
        let mut state_guard = self.state.write();
        let mut snapshot = state_guard.as_ref().clone();
        let popped = snapshot
            .imm_memtables
            .pop()
            .expect("should've been returned for empty");
        assert!(
            memtable_id == popped.id(),
            "last memtable changed while holding state_lock"
        );
        snapshot.l0_ss_tables.insert(0, memtable_id);
        snapshot.ss_tables.insert(memtable_id, sst);
        *state_guard = Arc::new(snapshot);
        self.sync_dir()?;
        Ok(())
    }

    pub fn new_txn(&self) -> Result<()> {
        // no-op
        Ok(())
    }

    /// Create an iterator over a range of keys.
    pub fn scan(
        &self,
        lower: Bound<&[u8]>,
        upper: Bound<&[u8]>,
    ) -> Result<FusedIterator<LsmIterator>> {
        let snapshot = {
            let state = self.state.read();
            Arc::clone(&state)
        };
        let mut memtable_iters = Vec::with_capacity(snapshot.imm_memtables.len() + 1);
        memtable_iters.push(Box::new(snapshot.memtable.scan(lower, upper)));
        for iter in &snapshot.imm_memtables {
            memtable_iters.push(Box::new(iter.scan(lower, upper)));
        }
        let mut l0_iters = Vec::with_capacity(snapshot.l0_ss_tables.len());
        for sst_id in &snapshot.l0_ss_tables {
            let sst = Arc::clone(&snapshot.ss_tables[sst_id]);
            if !range_overlap(
                lower,
                upper,
                sst.first_key().raw_ref(),
                sst.last_key().raw_ref(),
            ) {
                continue;
            }
            let sst_iter = match lower {
                Bound::Included(key) => {
                    SsTableIterator::create_and_seek_to_key(sst, KeySlice::from_slice(key))?
                }
                Bound::Excluded(key) => {
                    let mut iter =
                        SsTableIterator::create_and_seek_to_key(sst, KeySlice::from_slice(key))?;
                    if iter.is_valid() && iter.key().raw_ref() == key {
                        iter.next()?;
                    }
                    iter
                }
                Bound::Unbounded => SsTableIterator::create_and_seek_to_first(sst)?,
            };
            l0_iters.push(Box::new(sst_iter));
        }
        let mut concat_iters = Vec::with_capacity(snapshot.levels.len());
        for (_, level_sst_ids) in &snapshot.levels {
            let mut level_ssts = Vec::with_capacity(level_sst_ids.len());
            for sst_id in level_sst_ids {
                let sst = Arc::clone(&snapshot.ss_tables[sst_id]);
                if !range_overlap(
                    lower,
                    upper,
                    sst.first_key().raw_ref(),
                    sst.last_key().raw_ref(),
                ) {
                    continue;
                }
                level_ssts.push(sst);
            }
            let concat_iter = match lower {
                Bound::Included(key) => SstConcatIterator::create_and_seek_to_key(
                    level_ssts,
                    KeySlice::from_slice(key),
                )?,
                Bound::Excluded(key) => {
                    let mut iter = SstConcatIterator::create_and_seek_to_key(
                        level_ssts,
                        KeySlice::from_slice(key),
                    )?;
                    if iter.is_valid() && iter.key().raw_ref() == key {
                        iter.next()?;
                    }
                    iter
                }
                Bound::Unbounded => SstConcatIterator::create_and_seek_to_first(level_ssts)?,
            };
            concat_iters.push(Box::new(concat_iter));
        }
        let merge_iter = TwoMergeIterator::create(
            MergeIterator::create(memtable_iters),
            MergeIterator::create(l0_iters),
        )?;
        let inner_iter = TwoMergeIterator::create(merge_iter, MergeIterator::create(concat_iters))?;
        let lsm_iter = LsmIterator::new(inner_iter, map_bound(upper))?;
        Ok(FusedIterator::new(lsm_iter))
    }
}

fn range_overlap(
    user_begin: Bound<&[u8]>,
    user_end: Bound<&[u8]>,
    table_start: &[u8],
    table_end: &[u8],
) -> bool {
    match user_end {
        Bound::Included(key) if key < table_start => return false,
        Bound::Excluded(key) if key <= table_start => return false,
        _ => {}
    }
    match user_begin {
        Bound::Included(key) if key > table_end => return false,
        Bound::Excluded(key) if key >= table_end => return false,
        _ => {}
    }
    true
}

impl MiniLsm {
    pub fn new_txn(&self) -> Result<()> {
        self.inner.new_txn()
    }

    pub fn write_batch<T: AsRef<[u8]>>(&self, batch: &[WriteBatchRecord<T>]) -> Result<()> {
        self.inner.write_batch(batch)
    }

    pub fn get(&self, key: &[u8]) -> Result<Option<Bytes>> {
        self.inner.get(key)
    }

    pub fn put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        self.inner.put(key, value)
    }

    pub fn delete(&self, key: &[u8]) -> Result<()> {
        self.inner.delete(key)
    }

    pub fn sync(&self) -> Result<()> {
        self.inner.sync()
    }

    pub fn scan(
        &self,
        lower: Bound<&[u8]>,
        upper: Bound<&[u8]>,
    ) -> Result<FusedIterator<LsmIterator>> {
        self.inner.scan(lower, upper)
    }

    pub fn force_full_compaction(&self) -> Result<()> {
        self.inner.force_full_compaction()
    }

    pub fn add_compaction_filter(&self, compaction_filter: CompactionFilter) {
        self.inner.add_compaction_filter(compaction_filter)
    }
}

impl LsmStorageInner {
    pub(crate) fn mvcc(&self) -> &LsmMvccInner {
        self.mvcc.as_ref().unwrap()
    }

    pub(crate) fn path_of_sst_static(path: impl AsRef<Path>, id: usize) -> PathBuf {
        path.as_ref().join(format!("{:05}.sst", id))
    }

    pub(crate) fn path_of_sst(&self, id: usize) -> PathBuf {
        Self::path_of_sst_static(&self.path, id)
    }

    pub(crate) fn path_of_wal(&self, id: usize) -> PathBuf {
        Self::path_of_wal_static(&self.path, id)
    }

    pub(crate) fn path_of_wal_static(path: impl AsRef<Path>, id: usize) -> PathBuf {
        path.as_ref().join(format!("{:05}.wal", id))
    }
}
