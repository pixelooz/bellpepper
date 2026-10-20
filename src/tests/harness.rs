use std::{path::Path, sync::Arc};

use anyhow::{Result, bail};
use bytes::Bytes;

use crate::{
    iterators::StorageIterator,
    key::KeySlice,
    lsm_storage::BlockCache,
    table::{SsTable, builder::SsTableBuilder},
};

#[derive(Clone)]
pub struct MockIterator {
    pub data: Vec<(Bytes, Bytes)>,
    pub error_when: Option<usize>,
    pub index: usize,
}

impl MockIterator {
    pub fn new(data: Vec<(Bytes, Bytes)>) -> Self {
        Self {
            data,
            index: 0,
            error_when: None,
        }
    }

    pub fn new_with_error(data: Vec<(Bytes, Bytes)>, error_when: usize) -> Self {
        Self {
            data,
            index: 0,
            error_when: Some(error_when),
        }
    }
}

pub fn expect_iter_error(mut iter: impl StorageIterator) {
    loop {
        match iter.next() {
            Ok(_) if iter.is_valid() => continue,
            Ok(_) => panic!("expect an error"),
            Err(_) => break,
        }
    }
}

pub fn check_lsm_iter_result_by_key<I>(iter: &mut I, expected: Vec<(Bytes, Bytes)>)
where
    I: for<'a> StorageIterator<KeyType<'a> = &'a [u8]>,
{
    for (k, v) in expected {
        assert!(iter.is_valid());
        assert_eq!(
            k,
            iter.key(),
            "expected key: {:?}, actual key: {:?}",
            k,
            as_bytes(iter.key()),
        );
        assert_eq!(
            v,
            iter.value(),
            "expected value: {:?}, actual value: {:?}",
            v,
            as_bytes(iter.value()),
        );
        iter.next().unwrap();
    }
    assert!(!iter.is_valid());
}

impl StorageIterator for MockIterator {
    type KeyType<'a> = KeySlice<'a>;

    fn next(&mut self) -> Result<()> {
        if self.index < self.data.len() {
            self.index += 1;
        }
        if let Some(error_when) = self.error_when
            && self.index == error_when
        {
            bail!("fake error!");
        }
        Ok(())
    }

    fn key(&self) -> KeySlice<'_> {
        if let Some(error_when) = self.error_when
            && self.index >= error_when
        {
            panic!("invalid access after next returns an error!");
        }
        KeySlice::from_slice(self.data[self.index].0.as_ref())
    }

    fn value(&self) -> &[u8] {
        if let Some(error_when) = self.error_when
            && self.index >= error_when
        {
            panic!("invalid access after next returns an error!");
        }
        self.data[self.index].1.as_ref()
    }

    fn is_valid(&self) -> bool {
        if let Some(error_when) = self.error_when
            && self.index >= error_when
        {
            panic!("invalid access after next returns an error!");
        }
        self.index < self.data.len()
    }
}

pub fn generate_sst(
    id: usize,
    path: impl AsRef<Path>,
    data: Vec<(Bytes, Bytes)>,
    block_cache: Option<Arc<BlockCache>>,
) -> SsTable {
    let mut builder = SsTableBuilder::new(128);
    for (key, value) in data {
        builder.add(KeySlice::from_slice(&key[..]), &value[..]);
    }
    builder.build(path.as_ref(), id, block_cache).unwrap()
}

pub fn as_bytes(x: &[u8]) -> Bytes {
    Bytes::copy_from_slice(x)
}

pub fn check_iter_result_by_key<I>(iter: &mut I, expected: Vec<(Bytes, Bytes)>)
where
    I: for<'a> StorageIterator<KeyType<'a> = KeySlice<'a>>,
{
    for (k, v) in expected {
        assert!(iter.is_valid());
        assert_eq!(
            k,
            iter.key().raw_ref(),
            "expected key: {:?}, actual key: {:?}",
            k,
            as_bytes(iter.key().raw_ref()),
        );
        assert_eq!(
            v,
            iter.value(),
            "expected value: {:?}, actual value: {:?}",
            v,
            as_bytes(iter.value()),
        );
        iter.next().unwrap();
    }
    assert!(
        !iter.is_valid(),
        "iterator should not be valid at the end of the check"
    );
}
