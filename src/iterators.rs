pub mod merge_iterator;

use anyhow::Result;

pub trait StorageIterator {
    type KeyType<'a>: PartialEq + Eq + PartialOrd + Ord
    where
        Self: 'a;

    fn key(&self) -> Self::KeyType<'_>;

    fn value(&self) -> &[u8];

    fn is_valid(&self) -> bool;

    fn next(&mut self) -> Result<()>;

    fn num_active_iterators(&self) -> usize {
        1
    }
}
