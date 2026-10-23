use std::sync::Arc;

use anyhow::Result;

use super::StorageIterator;
use crate::{
    key::KeySlice,
    table::{SsTable, iterator::SsTableIterator},
};

/// Concat multiple iterators ordered in key order and their key ranges do not overlap.
/// We do not want to create the iterators when initializing this iterator to reduce
/// the overhead of seeking.
pub struct SstConcatIterator {
    current: Option<SsTableIterator>,
    next_sst_idx: usize,
    ss_tables: Vec<Arc<SsTable>>,
}

impl SstConcatIterator {
    pub fn create_and_seek_to_first(ss_tables: Vec<Arc<SsTable>>) -> Result<Self> {
        if ss_tables.is_empty() {
            let concat_iter = Self {
                next_sst_idx: 0,
                current: None,
                ss_tables,
            };
            return Ok(concat_iter);
        }
        Self::check_sst_valid(&ss_tables);
        let sst = Arc::clone(&ss_tables[0]);
        let iter = SsTableIterator::create_and_seek_to_first(sst)?;

        let mut concat_iter = Self {
            current: Some(iter),
            next_sst_idx: 1,
            ss_tables,
        };
        concat_iter.move_until_valid()?;
        Ok(concat_iter)
    }

    pub fn create_and_seek_to_key(ss_tables: Vec<Arc<SsTable>>, key: KeySlice) -> Result<Self> {
        if ss_tables.is_empty() {
            let concat_iter = Self {
                next_sst_idx: 0,
                current: None,
                ss_tables,
            };
            return Ok(concat_iter);
        }
        Self::check_sst_valid(&ss_tables);
        let index = ss_tables
            .partition_point(|sst| sst.first_key().as_key_slice() <= key)
            .saturating_sub(1);

        let sst = Arc::clone(&ss_tables[index]);
        let iter = SsTableIterator::create_and_seek_to_key(sst, key)?;

        let mut concat_iter = Self {
            next_sst_idx: index + 1,
            current: Some(iter),
            ss_tables,
        };
        concat_iter.move_until_valid()?;
        Ok(concat_iter)
    }

    fn move_until_valid(&mut self) -> Result<()> {
        while let Some(curr) = &self.current
            && !curr.is_valid()
        {
            if self.next_sst_idx >= self.ss_tables.len() {
                self.current = None;
                return Ok(());
            }
            let sst = Arc::clone(&self.ss_tables[self.next_sst_idx]);
            let iter = SsTableIterator::create_and_seek_to_first(sst)?;
            self.next_sst_idx += 1;
            self.current = Some(iter);
        }
        Ok(())
    }

    fn check_sst_valid(ss_tables: &[Arc<SsTable>]) {
        let ordered = ss_tables
            .iter()
            .all(|sst| sst.first_key() <= sst.last_key());
        assert!(ordered, "some SSTs are not internally ordered");

        let increasing = ss_tables
            .windows(2)
            .all(|sst| sst[0].last_key() < sst[1].first_key());
        assert!(increasing, "SSTs are not strictly increasing");
    }
}

impl StorageIterator for SstConcatIterator {
    type KeyType<'a> = KeySlice<'a>;

    fn key(&self) -> KeySlice<'_> {
        self.current.as_ref().unwrap().key()
    }

    fn value(&self) -> &[u8] {
        self.current.as_ref().unwrap().value()
    }

    fn is_valid(&self) -> bool {
        if let Some(curr) = &self.current {
            debug_assert!(curr.is_valid(), "inner iterator found to be not valid");
            return true;
        }
        false
    }

    fn next(&mut self) -> Result<()> {
        self.current.as_mut().unwrap().next()?;
        self.move_until_valid()
    }

    fn num_active_iterators(&self) -> usize {
        1
    }
}
