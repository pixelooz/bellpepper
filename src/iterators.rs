pub mod merge_iterator;

use anyhow::Result;

/// The unified interface for sequential data access across the entire LSM tree.
pub trait StorageIterator {
    /// The type of key yielded by the iterator.
    ///
    /// By using a Generic Associated Type (GAT) with `'a`, we allow the iterator to yield
    /// keys that are strictly tied to the lifetime of the `&self` borrow during `.key()`.
    /// In practice, implementations use `KeySlice<'a>`, which guarantees zero-copy access.
    type KeyType<'a>: PartialEq + Eq + PartialOrd + Ord
    where
        Self: 'a;

    /// Returns the key at the current iterator position.
    ///
    /// ***Note**: Calling this when `is_valid()` is false will typically panic.*
    fn key(&self) -> Self::KeyType<'_>;

    /// Returns the value at the current iterator position as a raw byte slice.
    ///
    /// ***Note**: Calling this when `is_valid()` is false will typically panic.*
    fn value(&self) -> &[u8];

    /// Returns `true` if the iterator is currently pointing to a valid key-value pair.
    /// This is the primary condition used in `while iter.is_valid() { ... }` loops.
    fn is_valid(&self) -> bool;

    /// Advances the iterator to the next key-value pair.
    ///
    /// Returns a `Result` because advancing the iterator might involve reading from disk,
    /// which can fail (e.g., I/O errors or checksum mismatches).
    fn next(&mut self) -> Result<()>;

    /// Returns the total number of underlying iterators currently active.
    fn num_active_iterators(&self) -> usize {
        1
    }
}
