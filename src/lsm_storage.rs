use std::{
    mem,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::Result;
use bytes::Bytes;
use parking_lot::{Mutex, MutexGuard, RwLock};

use crate::mem_table::MemTable;

/// Represents the state of the storage engine with the current memtable, all the imm_memtables,
/// and the ss_tables.
#[derive(Debug, Clone)]
pub(crate) struct LsmStorageState {
    pub(crate) memtable: Arc<MemTable>,
    pub(crate) imm_memtables: Vec<Arc<MemTable>>,
    pub(crate) l0_ss_tables: Vec<usize>,
}

impl LsmStorageState {
    pub fn create() -> Self {
        Self {
            memtable: Arc::new(MemTable::create(0)),
            imm_memtables: Vec::new(),
            l0_ss_tables: Vec::new(),
        }
    }
}

/// Options for initializing the Lsm storage engine.
#[derive(Debug, Clone)]
pub struct LsmStorageOptions {
    /// Block size in bytes.
    pub block_size: usize,

    /// SST size in bytes, and also the approximate size of memtables.
    pub target_sst_size: usize,

    /// Number of memtables allowed in memory before flushing them to L0.
    pub num_memtable_limit: usize,
}

impl LsmStorageOptions {
    pub fn default_for_tests() -> Self {
        Self {
            block_size: 4096,
            target_sst_size: 2 << 20,
            num_memtable_limit: 50,
        }
    }
}

/// Represents the interface for the Lsm storage engine providing the api to access
/// the state of storage engine and perform operations.
#[derive(Debug)]
pub(crate) struct LsmStorageInner {
    pub(crate) state: Arc<RwLock<Arc<LsmStorageState>>>,
    pub(crate) state_lock: Mutex<()>,
    path: PathBuf,
    next_sst_id: AtomicUsize,
    pub(crate) options: LsmStorageOptions,
}

impl LsmStorageInner {
    /// Opens the storage engine at the given path with the given options.
    pub fn open(path: impl AsRef<Path>, options: LsmStorageOptions) -> Self {
        let state = LsmStorageState::create();
        Self {
            state: Arc::new(RwLock::new(Arc::new(state))),
            state_lock: Mutex::new(()),
            path: path.as_ref().to_path_buf(),
            next_sst_id: AtomicUsize::new(1),
            options,
        }
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
        for mmt in &snapshot.imm_memtables {
            if let Some(value) = mmt.get(key) {
                if value.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(value));
            }
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
}
