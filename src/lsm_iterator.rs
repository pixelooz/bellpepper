use std::ops::Bound;

use crate::{
    iterators::{
        StorageIterator, concat_iterator::SstConcatIterator, merge_iterator::MergeIterator,
        two_merge_iterators::TwoMergeIterator,
    },
    mem_table::MemTableIterator,
    table::iterator::SsTableIterator,
};

use anyhow::{Result, bail};
use bytes::Bytes;

/// Represents the inner type for the `LsmIterator`.
type LsmIteratorInner = TwoMergeIterator<
    TwoMergeIterator<MergeIterator<MemTableIterator>, MergeIterator<SsTableIterator>>,
    MergeIterator<SstConcatIterator>,
>;

pub struct LsmIterator {
    inner: LsmIteratorInner,
    is_valid: bool,
    end_bound: Bound<Bytes>,
}

impl LsmIterator {
    /// Returns a newly instantiated LsmIterator.
    pub fn new(iter: LsmIteratorInner, end_bound: Bound<Bytes>) -> Result<LsmIterator> {
        let mut iter = Self {
            is_valid: iter.is_valid(),
            inner: iter,
            end_bound,
        };
        iter.check_end_bound();
        while iter.is_valid() && iter.inner.value().is_empty() {
            iter.next_inner()?;
        }
        Ok(iter)
    }

    fn check_end_bound(&mut self) {
        if !self.is_valid {
            return;
        }
        match self.end_bound.as_ref() {
            Bound::Unbounded => {}
            Bound::Included(key) => self.is_valid = self.inner.key().raw_ref() <= key.as_ref(),
            Bound::Excluded(key) => self.is_valid = self.inner.key().raw_ref() < key.as_ref(),
        }
    }

    /// Advances the inner iterator to the next value and sets the validity.
    fn next_inner(&mut self) -> Result<()> {
        self.inner.next()?;
        if !self.inner.is_valid() {
            self.is_valid = false;
            return Ok(());
        }
        self.check_end_bound();
        Ok(())
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
        while self.is_valid() && self.value().is_empty() {
            self.next_inner()?;
        }
        Ok(())
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
        Ok(())
    }

    fn num_active_iterators(&self) -> usize {
        self.iter.num_active_iterators()
    }
}
