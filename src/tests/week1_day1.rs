#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use crate::{lsm_storage::LsmStorageInner, mem_table::MemTable};

    #[test]
    fn test_task1_memtable_get() {
        let table = MemTable::create(0);

        table.put(b"key1", b"value1").unwrap();
        table.put(b"key2", b"value2").unwrap();
        table.put(b"key3", b"value3").unwrap();

        assert_eq!(&table.get(b"key1").unwrap()[..], b"value1");
        assert_eq!(&table.get(b"key2").unwrap()[..], b"value2");
        assert_eq!(&table.get(b"key3").unwrap()[..], b"value3");
    }

    #[test]
    fn test_task1_memtable_overwrite() {
        let table = MemTable::create(0);

        table.put(b"key1", b"value1").unwrap();
        table.put(b"key2", b"value2").unwrap();
        table.put(b"key3", b"value3").unwrap();

        table.put(b"key1", b"value11").unwrap();
        table.put(b"key2", b"value22").unwrap();
        table.put(b"key3", b"value33").unwrap();

        assert_eq!(&table.get(b"key1").unwrap()[..], b"value11");
        assert_eq!(&table.get(b"key2").unwrap()[..], b"value22");
        assert_eq!(&table.get(b"key3").unwrap()[..], b"value33");
    }

    #[test]
    fn test_task2_storage_integration() {
        let dir = PathBuf::from("../../task2_storage_integration");
        let storage = Arc::new(LsmStorageInner::open(dir));

        assert!(storage.get(b"0").unwrap().is_none());

        storage.put(b"1", b"233").unwrap();
        storage.put(b"2", b"2333").unwrap();
        storage.put(b"3", b"23333").unwrap();

        assert_eq!(&storage.get(b"1").unwrap().unwrap()[..], b"233");
        assert_eq!(&storage.get(b"2").unwrap().unwrap()[..], b"2333");
        assert_eq!(&storage.get(b"3").unwrap().unwrap()[..], b"23333");

        storage.delete(b"2").unwrap();
        assert!(storage.get(b"2").unwrap().is_none());
        storage.delete(b"0").unwrap();
    }
}
