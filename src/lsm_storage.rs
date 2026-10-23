use std::{
    collections::HashMap,
    fs::{self, File},
    mem,
    ops::Bound,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread::JoinHandle,
};

use anyhow::{Context, Result, anyhow, bail};
use bytes::Bytes;
use parking_lot::{Mutex, MutexGuard, RwLock};

use crate::{
    block::Block,
    iterators::{
        StorageIterator, concat_iterator::SstConcatIterator, merge_iterator::MergeIterator,
        two_merge_iterators::TwoMergeIterator,
    },
    key::KeySlice,
    lsm_iterator::{FusedIterator, LsmIterator},
    lsm_options::LsmStorageOptions,
    mem_table::{MemTable, map_bound},
    table::{SsTable, builder::SsTableBuilder, iterator::SsTableIterator},
};

/// A concurrent cache used for caching blocks to avoid reaching out to disk for commonly read blocks.
/// Uses SST id and block index as keys to locate exact blocks to read and store them as cache.
pub type BlockCache = moka::sync::Cache<(usize, usize), Arc<Block>>;

/// Represents the state of the storage engine with the current memtable, all the imm_memtables,
/// and the ss_tables.
#[derive(Debug, Clone)]
pub struct LsmStorageState {
    /// The current/active memtable.
    pub memtable: Arc<MemTable>,

    /// Immutable memtables, from latest to oldest.
    pub imm_memtables: Vec<Arc<MemTable>>,

    /// Level 0 SSTs, from latest to oldest.
    pub l0_ss_tables: Vec<usize>,

    /// SsTables sorted by key range; L1-L_MAX for leveled compaction or tiers for tiered
    /// compactions.
    pub levels: Vec<(usize, Vec<usize>)>,

    /// SST objects.
    pub ss_tables: HashMap<usize, Arc<SsTable>>,
}

impl LsmStorageState {
    pub fn create() -> Self {
        Self {
            memtable: Arc::new(MemTable::create(0)),
            imm_memtables: Vec::new(),
            l0_ss_tables: Vec::new(),
            levels: vec![(1, Vec::new())],
            ss_tables: HashMap::new(),
        }
    }
}

/// Checks whether than user bounds are within the SST's bounds.
fn sst_within_range(
    lower: Bound<&[u8]>,
    upper: Bound<&[u8]>,
    sst_lower: &KeySlice,
    sst_upper: &KeySlice,
) -> bool {
    match lower {
        Bound::Included(key) if key > sst_upper.raw_ref() => {
            return false;
        }
        Bound::Excluded(key) if key >= sst_upper.raw_ref() => {
            return false;
        }
        _ => {}
    }
    match upper {
        Bound::Included(key) if key < sst_lower.raw_ref() => {
            return false;
        }
        Bound::Excluded(key) if key <= sst_lower.raw_ref() => {
            return false;
        }
        _ => {}
    }
    true
}

/// A thin wrapper around `LsmStorageInner` and the user interface for the Database.
pub struct MiniLsm {
    pub(crate) inner: Arc<LsmStorageInner>,

    /// Notifies the l0 flush thread to stop working.
    flush_notifier: crossbeam_channel::Sender<()>,

    /// The handle for the flush thread.
    flush_thread: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for MiniLsm {
    fn drop(&mut self) {
        self.flush_notifier.send(()).ok();
    }
}

impl MiniLsm {
    /// Start the storage engine by either loading an existing directory or creating a new
    /// one if one doesn't exist.
    pub fn open(path: impl AsRef<Path>, options: LsmStorageOptions) -> Result<Arc<Self>> {
        let inner = Arc::new(LsmStorageInner::open(path, options)?);
        let (tx, rx) = crossbeam_channel::unbounded();
        let flush_thread = inner.spawn_flush_thread(rx)?;
        Ok(Arc::new(Self {
            inner,
            flush_notifier: tx,
            flush_thread: Mutex::new(flush_thread),
        }))
    }

    /// Shuts down the flush and compaction thread by sending their receivers a message.
    pub fn close(&self) -> Result<()> {
        self.inner.sync_dir()?;
        self.flush_notifier.send(()).ok();
        let handle = self.flush_thread.lock().take();
        if let Some(handle) = handle {
            handle.join().map_err(|err| anyhow!("{:?}", err))?
        }
        Ok(())
    }

    pub fn put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        self.inner.put(key, value)
    }

    pub fn get(&self, key: &[u8]) -> Result<Option<Bytes>> {
        self.inner.get(key)
    }

    pub fn delete(&self, key: &[u8]) -> Result<()> {
        self.inner.delete(key)
    }

    pub fn scan(
        &self,
        lower: Bound<&[u8]>,
        upper: Bound<&[u8]>,
    ) -> Result<FusedIterator<LsmIterator>> {
        self.inner.scan(lower, upper)
    }

    /// Only to be used in tests and cli demo.
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

    pub fn force_full_compaction(&self) -> Result<()> {
        self.inner.force_full_compaction()
    }
}

/// Represents the interface for the Lsm storage engine providing the api to access
/// the state of storage engine and perform operations.
#[derive(Debug)]
pub(crate) struct LsmStorageInner {
    pub(crate) state: Arc<RwLock<Arc<LsmStorageState>>>,
    pub(crate) state_lock: Mutex<()>,
    path: PathBuf,
    pub(crate) block_cache: Arc<BlockCache>,
    next_sst_id: AtomicUsize,
    pub(crate) options: LsmStorageOptions,
}

impl LsmStorageInner {
    /// Opens the storage engine at the given path with the given options.
    pub fn open(path: impl AsRef<Path>, options: LsmStorageOptions) -> Result<Self> {
        if !path.as_ref().exists() {
            fs::create_dir_all(&path).context("failed to create DB dir")?;
        }
        let state = LsmStorageState::create();
        let block_cache = Arc::new(BlockCache::new(1 << 20));
        Ok(Self {
            state: Arc::new(RwLock::new(Arc::new(state))),
            state_lock: Mutex::new(()),
            path: path.as_ref().to_path_buf(),
            block_cache,
            next_sst_id: AtomicUsize::new(1),
            options,
        })
    }

    /// Syncs the current file to disk.
    pub(super) fn sync_dir(&self) -> Result<()> {
        File::open(&self.path)?.sync_all()?;
        Ok(())
    }

    pub fn new_txn(&self) -> Result<()> {
        Ok(())
    }

    /// Returns the next sst_id for the requested memtable.
    pub(crate) fn next_sst_id(&self) -> usize {
        self.next_sst_id.fetch_add(1, Ordering::SeqCst)
    }

    pub(crate) fn path_of_sst_static(path: impl AsRef<Path>, id: usize) -> PathBuf {
        path.as_ref().join(format!("{:05}.sst", id))
    }

    pub(crate) fn path_of_sst(&self, id: usize) -> PathBuf {
        Self::path_of_sst_static(&self.path, id)
    }

    pub(crate) fn path_of_wal_static(path: impl AsRef<Path>, id: usize) -> PathBuf {
        path.as_ref().join(format!("{:05}.wal", id))
    }

    pub(crate) fn path_of_wal(&self, id: usize) -> PathBuf {
        Self::path_of_wal_static(&self.path, id)
    }

    /// Puts the given key value pair in the memtable and attempts to freeze the table if
    /// size exceeds the threshold.
    pub fn put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        let approx_size = {
            let state = self.state.read();
            state.memtable.put(key, value)?;
            state.memtable.approximate_size()
        };
        self.try_freeze(approx_size)?;
        Ok(())
    }

    /// Returns the value for the given key as Some(x) or None if not present.
    /// If the current memtable doesn't have the value, looks for it in the
    /// immutable memtables.
    pub fn get(&self, search_key: &[u8]) -> Result<Option<Bytes>> {
        let snapshot = {
            let guard = self.state.read();
            Arc::clone(&guard)
        };
        if let Some(value) = snapshot.memtable.get(search_key) {
            if value.is_empty() {
                return Ok(None);
            }
            return Ok(Some(value));
        }
        for mmt in &snapshot.imm_memtables {
            if let Some(value) = mmt.get(search_key) {
                if value.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(value));
            }
        }
        let mut sst_iters = Vec::with_capacity(snapshot.l0_ss_tables.len());
        let hashed_key = farmhash::fingerprint32(search_key);
        for sst_id in &snapshot.l0_ss_tables {
            let Some(ss_table) = snapshot.ss_tables.get(sst_id).cloned() else {
                bail!("SST doesn't exist for SST ID: {sst_id}")
            };
            if search_key < ss_table.first_key().raw_ref()
                || search_key > ss_table.last_key().raw_ref()
            {
                continue;
            }
            if let Some(bloom) = &ss_table.bloom
                && !bloom.may_contain(hashed_key)
            {
                continue;
            }
            let iter = SsTableIterator::create_and_seek_to_key(
                ss_table,
                KeySlice::from_slice(search_key),
            )?;
            sst_iters.push(Box::new(iter));
        }
        let sst_merge_iter = MergeIterator::create(sst_iters);

        let mut lvl_iters = Vec::with_capacity(snapshot.levels.len());
        for (_, lvl_sst_ids) in &snapshot.levels {
            let mut lvl_ssts = Vec::with_capacity(lvl_sst_ids.len());
            for sst_id in lvl_sst_ids {
                let Some(sst) = snapshot.ss_tables.get(sst_id).cloned() else {
                    bail!("SST does not exist for ID: {sst_id}");
                };
                if search_key < sst.first_key().raw_ref() || search_key > sst.last_key().raw_ref() {
                    continue;
                }
                if let Some(bloom) = &sst.bloom
                    && !bloom.may_contain(hashed_key)
                {
                    continue;
                }
                lvl_ssts.push(sst);
            }
            let concat_iter = SstConcatIterator::create_and_seek_to_key(
                lvl_ssts,
                KeySlice::from_slice(search_key),
            )?;
            lvl_iters.push(Box::new(concat_iter));
        }
        let two_merge_iter =
            TwoMergeIterator::create(sst_merge_iter, MergeIterator::create(lvl_iters))?;
        if two_merge_iter.is_valid() && two_merge_iter.key().raw_ref() == search_key {
            let value = two_merge_iter.value();
            if value.is_empty() {
                return Ok(None);
            }
            return Ok(Some(Bytes::copy_from_slice(value)));
        }
        Ok(None)
    }

    /// Logically deletes the value for the given key by "replacing" the value with empty
    /// value.
    pub fn delete(&self, key: &[u8]) -> Result<()> {
        self.put(key, b"")
    }

    /// Attempts to freeze the current memtable if estimated size exceeds threshold.
    pub fn try_freeze(&self, estimated_size: usize) -> Result<()> {
        if estimated_size >= self.options.target_sst_size {
            let state_lock_guard = self.state_lock.lock();
            let state_guard = self.state.read();
            if state_guard.memtable.approximate_size() >= self.options.target_sst_size {
                drop(state_guard);
                self.force_freeze_memtable(&state_lock_guard)?;
            }
        }
        Ok(())
    }

    /// Freezes the current memtable and replaces it with a fresh memtable, and inserts
    /// the old one into `imm_memtables`.
    pub fn force_freeze_memtable(&self, _state_lock_observer: &MutexGuard<()>) -> Result<()> {
        let memtable_id = self.next_sst_id();
        let new_memtable = Arc::new(MemTable::create(memtable_id));

        let mut state_guard = self.state.write();
        let mut snapshot = state_guard.as_ref().clone();

        let old_memtable = mem::replace(&mut snapshot.memtable, new_memtable);

        snapshot.imm_memtables.insert(0, old_memtable);
        *state_guard = Arc::new(snapshot);
        Ok(())
    }

    /// Force flush the earliest-created immutable memtable to disk
    pub fn force_flush_next_imm_memtable(&self) -> Result<()> {
        let _state_lock = self.state_lock.lock();

        let last_memtable = {
            let guard = self.state.read();
            let Some(mmt) = guard.imm_memtables.last() else {
                return Ok(());
            };
            Arc::clone(mmt)
        };
        let mut builder = SsTableBuilder::new(self.options.block_size);
        last_memtable.flush(&mut builder)?;

        let sst_id = last_memtable.id();
        let sst = Arc::new(builder.build(
            self.path_of_sst(sst_id),
            sst_id,
            Some(self.block_cache.clone()),
        )?);
        self.sync_dir()?;
        let mut state_guard = self.state.write();
        let mut snapshot = state_guard.as_ref().clone();

        let memtable = snapshot
            .imm_memtables
            .pop()
            .expect("empty should've returned above");

        assert_eq!(
            memtable.id(),
            sst_id,
            "last SST ID is not equal to popped SST ID"
        );
        snapshot.l0_ss_tables.insert(0, sst_id);
        snapshot.ss_tables.insert(sst_id, sst);
        *state_guard = Arc::new(snapshot);
        Ok(())
    }

    /// Creates a point-in-time snapshot iterator over the specified key range.
    /// It merges all in-memory tables, yielding the latest version of each key while skipping tombstones.
    /// The returned `FusedIterator` is safe to use in a loop and will permanently halt if an error occurs.
    pub fn scan(
        &self,
        lower: Bound<&[u8]>,
        upper: Bound<&[u8]>,
    ) -> Result<FusedIterator<LsmIterator>> {
        let snapshot = {
            let guard = self.state.read();
            Arc::clone(&guard)
        };
        let mut memtable_iters = Vec::with_capacity(snapshot.imm_memtables.len() + 1);
        memtable_iters.push(Box::new(snapshot.memtable.scan(lower, upper)));

        for mmt in &snapshot.imm_memtables {
            memtable_iters.push(Box::new(mmt.scan(lower, upper)));
        }
        let merged_mmt_iter = MergeIterator::create(memtable_iters);

        let mut sst_iters = Vec::with_capacity(snapshot.l0_ss_tables.len());
        for sst_id in &snapshot.l0_ss_tables {
            let Some(ss_table) = snapshot.ss_tables.get(sst_id).cloned() else {
                bail!("SST doesn't exist for SST ID: {sst_id}")
            };
            if !sst_within_range(
                lower,
                upper,
                &ss_table.first_key().as_key_slice(),
                &ss_table.last_key().as_key_slice(),
            ) {
                continue;
            }
            let sst_iter = match lower {
                Bound::Excluded(key) => {
                    let key = KeySlice::from_slice(key);
                    let mut iter = SsTableIterator::create_and_seek_to_key(ss_table, key)?;
                    if iter.is_valid() && key == iter.key() {
                        iter.next()?;
                    }
                    iter
                }
                Bound::Included(key) => {
                    let key = KeySlice::from_slice(key);
                    SsTableIterator::create_and_seek_to_key(ss_table, key)?
                }
                Bound::Unbounded => SsTableIterator::create_and_seek_to_first(ss_table)?,
            };
            sst_iters.push(Box::new(sst_iter));
        }
        let merged_sst_iter = MergeIterator::create(sst_iters);

        let mut lvl_iters = Vec::with_capacity(snapshot.levels.len());
        for (_, lvl_sst_ids) in &snapshot.levels {
            let mut lvl_ssts = Vec::with_capacity(lvl_sst_ids.len());
            for sst_id in lvl_sst_ids {
                let Some(sst) = snapshot.ss_tables.get(sst_id).cloned() else {
                    bail!("SST does not exist for ID: {sst_id}");
                };
                if !sst_within_range(
                    lower,
                    upper,
                    &sst.first_key().as_key_slice(),
                    &sst.last_key().as_key_slice(),
                ) {
                    continue;
                }
                lvl_ssts.push(sst);
            }
            let concat_iter = match lower {
                Bound::Excluded(key) => {
                    let key = KeySlice::from_slice(key);
                    let mut iter = SstConcatIterator::create_and_seek_to_key(lvl_ssts, key)?;
                    while iter.is_valid() && iter.key() == key {
                        iter.next()?;
                    }
                    iter
                }
                Bound::Included(key) => {
                    let key = KeySlice::from_slice(key);
                    SstConcatIterator::create_and_seek_to_key(lvl_ssts, key)?
                }
                Bound::Unbounded => SstConcatIterator::create_and_seek_to_first(lvl_ssts)?,
            };
            lvl_iters.push(Box::new(concat_iter));
        }
        let iter = TwoMergeIterator::create(merged_mmt_iter, merged_sst_iter)?;
        let iter = TwoMergeIterator::create(iter, MergeIterator::create(lvl_iters))?;
        Ok(FusedIterator::new(LsmIterator::new(
            iter,
            map_bound(upper),
        )?))
    }
}
