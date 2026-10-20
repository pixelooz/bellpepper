use std::{
    collections::HashMap,
    mem,
    ops::Bound,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::{Result, bail};
use bytes::Bytes;
use parking_lot::{Mutex, MutexGuard, RwLock};

use crate::{
    block::Block,
    iterators::{
        StorageIterator, merge_iterator::MergeIterator, two_merge_iterators::TwoMergeIterator,
    },
    key::KeySlice,
    lsm_iterator::{FusedIterator, LsmIterator},
    lsm_options::LsmStorageOptions,
    mem_table::{MemTable, map_bound},
    table::{SsTable, iterator::SsTableIterator},
};

/// A concurrent cache used for caching blocks to avoid reaching out to disk for commonly read blocks.
/// Uses SST id and block index as keys to locate exact blocks to read and store them as cache.
pub type BlockCache = moka::sync::Cache<(usize, usize), Arc<Block>>;

/// Represents the state of the storage engine with the current memtable, all the imm_memtables,
/// and the ss_tables.
#[derive(Debug, Clone)]
pub(crate) struct LsmStorageState {
    /// The current/active memtable.
    pub memtable: Arc<MemTable>,

    /// Immutable memtables, from latest to oldest.
    pub imm_memtables: Vec<Arc<MemTable>>,

    /// Level 0 SSTs, from latest to oldest.
    pub l0_ss_tables: Vec<usize>,

    /// SST objects.
    pub ss_tables: HashMap<usize, Arc<SsTable>>,
}

impl LsmStorageState {
    pub fn create() -> Self {
        Self {
            memtable: Arc::new(MemTable::create(0)),
            imm_memtables: Vec::new(),
            l0_ss_tables: Vec::new(),
            ss_tables: Default::default(),
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

    /// Returns the next sst_id for the requested memtable.
    pub(crate) fn next_sst_id(&self) -> usize {
        self.next_sst_id.fetch_add(1, Ordering::SeqCst)
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
        for sst_id in &snapshot.l0_ss_tables {
            let Some(ss_table) = snapshot.ss_tables.get(sst_id).cloned() else {
                bail!("SST doesn't exist for SST ID: {sst_id}")
            };
            if search_key < ss_table.first_key().raw_ref()
                || search_key > ss_table.last_key().raw_ref()
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
        if sst_merge_iter.is_valid() && sst_merge_iter.key().raw_ref() == search_key {
            let value = sst_merge_iter.value();
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
        let two_merge_iter = TwoMergeIterator::create(merged_mmt_iter, merged_sst_iter)?;
        let lsm_iter = LsmIterator::new(two_merge_iter, map_bound(upper))?;
        let fused_iter = FusedIterator::new(lsm_iter);
        Ok(fused_iter)
    }
}
