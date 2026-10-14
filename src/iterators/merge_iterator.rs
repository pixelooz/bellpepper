use std::{
    cmp::Ordering,
    collections::{BinaryHeap, binary_heap::PeekMut},
    mem,
};

use anyhow::Result;

use crate::{iterators::StorageIterator, key::KeySlice};

/// A thin wrapper around the `StorageIterator` type, with an index to keep track of iterator hierarchies.
pub struct HeapWrapper<I: StorageIterator>(pub usize, pub Box<I>);

impl<I: StorageIterator> PartialEq for HeapWrapper<I> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<I: StorageIterator> Eq for HeapWrapper<I> {}

impl<I: StorageIterator> PartialOrd for HeapWrapper<I> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<I: StorageIterator> Ord for HeapWrapper<I> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.1
            .key()
            .cmp(&other.1.key())
            .then(self.0.cmp(&other.0))
            .reverse()
    }
}

/// A combination of multiple iterators merged together in sorted order, resolving key
/// conflicts by choosing the one with the smallest index.
pub struct MergeIterator<I: StorageIterator> {
    iterators: BinaryHeap<HeapWrapper<I>>,
    current: Option<HeapWrapper<I>>,
}

impl<I: StorageIterator> MergeIterator<I> {
    /// Creates a `MergeIterator` from the given vec of iterators handling empty/invalid
    /// iterators in the process.
    pub fn create(iterators: Vec<Box<I>>) -> Self {
        if iterators.is_empty() {
            return Self {
                iterators: BinaryHeap::new(),
                current: None,
            };
        }
        let mut heap = BinaryHeap::new();

        if iterators.iter().all(|x| !x.is_valid()) {
            // Not one iterator is valid (has items).
            let mut iter = iterators;
            return Self {
                iterators: heap,
                current: Some(HeapWrapper(
                    0,
                    iter.pop().expect("empty should've been caught"),
                )),
            };
        }
        for (idx, iter) in iterators.into_iter().enumerate() {
            if iter.is_valid() {
                heap.push(HeapWrapper(idx, iter));
            }
        }
        let curr = heap.pop().expect("should not be None");
        Self {
            iterators: heap,
            current: Some(curr),
        }
    }
}

impl<I: 'static + for<'a> StorageIterator<KeyType<'a> = KeySlice<'a>>> StorageIterator
    for MergeIterator<I>
{
    type KeyType<'a> = KeySlice<'a>;

    fn key(&self) -> Self::KeyType<'_> {
        self.current
            .as_ref()
            .expect("`is_valid` should've caught None")
            .1
            .key()
    }

    fn value(&self) -> &[u8] {
        self.current
            .as_ref()
            .expect("`is_valid` should've caught None")
            .1
            .value()
    }

    fn is_valid(&self) -> bool {
        self.current
            .as_ref()
            .map(|x| x.1.is_valid())
            .unwrap_or(false)
    }

    fn next(&mut self) -> Result<()> {
        let curr_iter = self
            .current
            .as_mut()
            .expect("`is_valid` should've caught None");

        while let Some(mut inner_iter) = self.iterators.peek_mut() {
            debug_assert!(
                inner_iter.1.key() >= curr_iter.1.key(),
                "heap invariant violated"
            );
            if curr_iter.1.key() == inner_iter.1.key() {
                if let err @ Err(_) = inner_iter.1.next() {
                    PeekMut::pop(inner_iter);
                    return err;
                }
                if !inner_iter.1.is_valid() {
                    PeekMut::pop(inner_iter);
                }
            } else {
                break;
            }
        }
        curr_iter.1.next()?;

        if !curr_iter.1.is_valid() {
            if let Some(iter) = self.iterators.pop() {
                *curr_iter = iter;
            }
            return Ok(());
        }
        if let Some(mut inner_iter) = self.iterators.peek_mut()
            && *curr_iter < *inner_iter
        {
            mem::swap(curr_iter, &mut *inner_iter);
        }
        Ok(())
    }

    fn num_active_iterators(&self) -> usize {
        let total_iters = self
            .iterators
            .iter()
            .map(|x| x.1.num_active_iterators())
            .sum::<usize>();

        let current_iter = self
            .current
            .as_ref()
            .map(|x| x.1.num_active_iterators())
            .unwrap_or(0);

        total_iters + current_iter
    }
}
