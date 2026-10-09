use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;
use bytes::Bytes;
use crossbeam_skiplist::SkipMap;

/// The smallest block of the Lsm Tree database. It is built using SkipList which is
/// a concurrent map essentially of `Bytes`. Keeps track of its size and when size
/// exceeds the threshold it is written to the disk to the SSTable in a background process.
/// The `Wal` ensures that written data is never lost due to a crash.
#[derive(Debug)]
pub(crate) struct MemTable {
    map: Arc<SkipMap<Bytes, Bytes>>,
    // wal: Option<Wal>,
    id: usize,
    approximate_size: AtomicUsize,
}

impl MemTable {
    /// Creates a new empty `MemTable`.
    pub fn create(id: usize) -> Self {
        Self {
            map: Arc::new(SkipMap::new()),
            id,
            approximate_size: AtomicUsize::new(0),
        }
    }

    /// Returns the current approximate size of the memtable.
    pub fn approximate_size(&self) -> usize {
        self.approximate_size.load(Ordering::Relaxed)
    }

    /// Puts the given key/value pair into the map and then increments the size of the MemTable.
    pub fn put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        let approx_size = key.len() + value.len();
        self.map
            .insert(Bytes::copy_from_slice(key), Bytes::copy_from_slice(value));
        self.approximate_size
            .fetch_add(approx_size, Ordering::Relaxed);
        Ok(())
    }

    /// Returns the value for the given key if exists otherwise returns `None`.
    pub fn get(&self, key: &[u8]) -> Option<Bytes> {
        self.map.get(key).map(|e| e.value().clone())
    }
}
