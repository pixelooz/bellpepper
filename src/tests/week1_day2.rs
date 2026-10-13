#[cfg(test)]
mod tests {
    use std::ops::Bound;

    use crate::{lsm_iterator::StorageIterator, mem_table::MemTable};

    #[test]
    fn test_task1_memtable_iter() {
        let memtable = MemTable::create(0);
        memtable.put(b"key1", b"value1").unwrap();
        memtable.put(b"key2", b"value2").unwrap();
        memtable.put(b"key3", b"value3").unwrap();
        {
            let mut iter = memtable.scan(Bound::Unbounded, Bound::Unbounded);

            assert_eq!(iter.key().raw_ref(), b"key1");
            assert_eq!(iter.value(), b"value1");
            assert!(iter.is_valid());
            iter.next().unwrap();

            assert_eq!(iter.key().raw_ref(), b"key2");
            assert_eq!(iter.value(), b"value2");
            assert!(iter.is_valid());
            iter.next().unwrap();

            assert_eq!(iter.key().raw_ref(), b"key3");
            assert_eq!(iter.value(), b"value3");
            assert!(iter.is_valid());
            iter.next().unwrap();

            assert!(!iter.is_valid());
        }
        {
            let mut iter = memtable.scan(Bound::Included(b"key1"), Bound::Included(b"key2"));

            assert_eq!(iter.key().raw_ref(), b"key1");
            assert_eq!(iter.value(), b"value1");
            assert!(iter.is_valid());
            iter.next().unwrap();

            assert_eq!(iter.key().raw_ref(), b"key2");
            assert_eq!(iter.value(), b"value2");
            assert!(iter.is_valid());
            iter.next().unwrap();

            assert!(!iter.is_valid());
        }
        {
            let mut iter = memtable.scan(Bound::Excluded(b"key1"), Bound::Excluded(b"key3"));

            assert_eq!(iter.key().raw_ref(), b"key2");
            assert_eq!(iter.value(), b"value2");
            assert!(iter.is_valid());
            iter.next().unwrap();

            assert!(!iter.is_valid());
        }
    }

    #[test]
    fn test_task1_empty_memtable_iter() {
        use std::ops::Bound;
        let memtable = MemTable::create(0);
        {
            let iter = memtable.scan(Bound::Excluded(b"key1"), Bound::Excluded(b"key3"));
            assert!(!iter.is_valid());
        }
        {
            let iter = memtable.scan(Bound::Included(b"key1"), Bound::Included(b"key2"));
            assert!(!iter.is_valid());
        }
        {
            let iter = memtable.scan(Bound::Unbounded, Bound::Unbounded);
            assert!(!iter.is_valid());
        }
    }
}
