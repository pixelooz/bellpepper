use crate::{
    iterators::{StorageIterator, merge_iterator::MergeIterator},
    mem_table::MemTableIterator,
};

use anyhow::{Result, bail};

/// Represents the inner type for the `LsmIterator`.
type LsmIteratorInner = MergeIterator<MemTableIterator>;

pub struct LsmIterator {
    inner: LsmIteratorInner,
    is_valid: bool,
}

impl LsmIterator {
    /// Returns a newly instantiated LsmIterator.
    pub fn new(iter: LsmIteratorInner) -> LsmIterator {
        Self {
            is_valid: iter.is_valid(),
            inner: iter,
        }
    }

    /// Advances the inner iterator to the next value and sets the validity.
    fn next_inner(&mut self) -> Result<()> {
        self.inner.next()?;
        if !self.inner.is_valid() {
            self.is_valid = false;
            return Ok(());
        }
        return Ok(());
    }
}

impl StorageIterator for LsmIterator {
    type KeyType<'a> = &'a [u8];

    fn key(&self) -> Self::KeyType<'_> {
        self.inner.key().raw_ref()
    }

    fn value(&self) -> &[u8] {
        self.inner.value()
    }

    fn is_valid(&self) -> bool {
        self.is_valid
    }

    fn next(&mut self) -> Result<()> {
        self.next_inner()?;
        while self.inner.is_valid() && self.inner.value().is_empty() {
            self.next_inner()?;
        }
        return Ok(());
    }

    fn num_active_iterators(&self) -> usize {
        self.inner.num_active_iterators()
    }
}

/// A wrapper around existing iterator, will prevent users from calling `next` when
/// iterator is invalid. When an iterator is already invalid `next` does not do
/// anything. If `next` returns an error, `is_valid` should return false, and `next`
/// should always return an error.
pub struct FusedIterator<I: StorageIterator> {
    iter: I,
    has_errored: bool,
}

impl<I: StorageIterator> FusedIterator<I> {
    /// Creates a new FusedIterator.
    pub fn new(iter: I) -> Self {
        Self {
            iter,
            has_errored: false,
        }
    }
}

impl<I: StorageIterator> StorageIterator for FusedIterator<I> {
    type KeyType<'a>
        = I::KeyType<'a>
    where
        Self: 'a;

    fn key(&self) -> Self::KeyType<'_> {
        if !self.is_valid() {
            panic!("invalid access to underlying iterator");
        }
        self.iter.key()
    }

    fn value(&self) -> &[u8] {
        if !self.is_valid() {
            panic!("invalid access to underlying iterator");
        }
        self.iter.value()
    }

    fn is_valid(&self) -> bool {
        !self.has_errored && self.iter.is_valid()
    }

    fn next(&mut self) -> Result<()> {
        if self.has_errored {
            bail!("iterator is tainted");
        }
        if self.iter.is_valid()
            && let Err(err) = self.iter.next()
        {
            self.has_errored = true;
            return Err(err);
        }
        return Ok(());
    }

    fn num_active_iterators(&self) -> usize {
        self.iter.num_active_iterators()
    }
}
