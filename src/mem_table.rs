use std::{
    mem,
    ops::Bound,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::Result;
use bytes::Bytes;
use crossbeam_skiplist::{SkipMap, map};

use crate::{iterators::StorageIterator, key::KeySlice, table::builder::SsTableBuilder};

/// The smallest block of the Lsm Tree database. It is built using SkipList which is
/// a concurrent map essentially of `Bytes`. Keeps track of its size and when size
/// exceeds the threshold it is written to the disk to the SSTable in a background process.
/// The `Wal` ensures that written data is never lost due to a crash.
#[derive(Debug)]
pub struct MemTable {
    map: Arc<SkipMap<Bytes, Bytes>>,
    // wal: Option<Wal>,
    id: usize,
    approximate_size: AtomicUsize,
}

/// Create a `Bound` of `Bytes` from `&[u8]`.
pub(crate) fn map_bound(bound: Bound<&[u8]>) -> Bound<Bytes> {
    match bound {
        Bound::Included(x) => Bound::Included(Bytes::copy_from_slice(x)),
        Bound::Excluded(x) => Bound::Excluded(Bytes::copy_from_slice(x)),
        Bound::Unbounded => Bound::Unbounded,
    }
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

    /// The id of the MemTable.
    pub fn id(&self) -> usize {
        self.id
    }

    /// Memtable map empty or not.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
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

    pub fn put_batch(&self, data: &[(KeySlice, &[u8])]) -> Result<()> {
        unimplemented!()
    }

    /// Flush the mem-table to SSTable.
    pub fn flush(&self, builder: &mut SsTableBuilder) -> Result<()> {
        for entry in self.map.iter() {
            builder.add(KeySlice::from_slice(entry.key()), entry.value());
        }
        Ok(())
    }

    /// Constructs a `MemTableIterator` on the active SkipMap given the lower and upper bound.
    pub fn scan(&self, lower: Bound<&[u8]>, upper: Bound<&[u8]>) -> MemTableIterator {
        let (lower, upper) = (map_bound(lower), map_bound(upper));
        let map = Arc::clone(&self.map);
        let iter = map.range((lower, upper));

        let static_iter =
            unsafe { mem::transmute::<SkipMapRangeIter<'_>, SkipMapRangeIter<'static>>(iter) };

        let mut iter = MemTableIterator {
            iter: static_iter,
            map,
            item: (Bytes::new(), Bytes::new()),
        };
        iter.next().unwrap();
        iter
    }
}

/// Type alias for `crossbeam_skiplist::map::Range`.
type SkipMapRangeIter<'a> = map::Range<'a, Bytes, (Bound<Bytes>, Bound<Bytes>), Bytes, Bytes>;

/// An iterator over a range of `SkipMap`.
pub struct MemTableIterator {
    iter: SkipMapRangeIter<'static>,
    map: Arc<SkipMap<Bytes, Bytes>>,
    item: (Bytes, Bytes),
}

impl StorageIterator for MemTableIterator {
    type KeyType<'a> = KeySlice<'a>;

    fn key(&self) -> Self::KeyType<'_> {
        KeySlice::from_slice(&self.item.0[..])
    }

    fn value(&self) -> &[u8] {
        &self.item.1[..]
    }

    fn is_valid(&self) -> bool {
        !self.item.0[..].is_empty()
    }

    fn next(&mut self) -> Result<()> {
        let entry = self.iter.next();
        self.item = entry
            .map(|x| (x.key().clone(), x.value().clone()))
            .unwrap_or_else(|| (Bytes::from_static(&[]), Bytes::from_static(&[])));
        Ok(())
    }
}
