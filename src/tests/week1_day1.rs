#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use crate::{
        lsm_storage::{LsmStorageInner, LsmStorageOptions},
        mem_table::MemTable,
    };

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
        let opts = LsmStorageOptions::default_for_tests();
        let storage = Arc::new(LsmStorageInner::open(dir, opts));

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

    #[test]
    fn test_task3_storage_integration() {
        let dir = PathBuf::from("../../task2_storage_integration");
        let opts = LsmStorageOptions::default_for_tests();
        let storage = Arc::new(LsmStorageInner::open(dir, opts));

        storage.put(b"1", b"233").unwrap();
        storage.put(b"2", b"2333").unwrap();
        storage.put(b"3", b"23333").unwrap();

        storage
            .force_freeze_memtable(&storage.state_lock.lock())
            .unwrap();
        assert_eq!(storage.state.read().imm_memtables.len(), 1);

        let previous_size = storage.state.read().imm_memtables[0].approximate_size();
        assert!(previous_size >= 15);

        storage.put(b"1", b"2333").unwrap();
        storage.put(b"2", b"23333").unwrap();
        storage.put(b"3", b"233333").unwrap();

        storage
            .force_freeze_memtable(&storage.state_lock.lock())
            .unwrap();
        assert_eq!(storage.state.read().imm_memtables.len(), 2);

        // This will access the same memtable that `previous_size` does cause it goes from newest to oldest.
        assert!(
            storage.state.read().imm_memtables[1].approximate_size() == previous_size,
            "wrong order of memtables?"
        );
        assert!(storage.state.read().imm_memtables[0].approximate_size() > previous_size);
    }

    #[test]
    fn test_task3_free_on_capacity() {
        let mut opts = LsmStorageOptions::default_for_tests();
        opts.target_sst_size = 1024;
        opts.num_memtable_limit = 1000;

        let dir = PathBuf::from("../../task2_storage_integration");
        let storage = Arc::new(LsmStorageInner::open(dir, opts));

        for _ in 0..1000 {
            storage.put(b"1", b"2333").unwrap();
        }
        let num_imm_memtables = storage.state.read().imm_memtables.len();
        assert!(num_imm_memtables >= 1, "no memtable frozen?");

        for _ in 0..1000 {
            storage.delete(b"1").unwrap();
        }
        assert!(
            storage.state.read().imm_memtables.len() > num_imm_memtables,
            "no memtable frozen?"
        );
    }

    #[test]
    fn test_task4_storage_integration() {
        let dir = PathBuf::from("../../task2_storage_integration");
        let opts = LsmStorageOptions::default_for_tests();
        let storage = Arc::new(LsmStorageInner::open(dir, opts));
        assert_eq!(&storage.get(b"0").unwrap(), &None);
        storage.put(b"1", b"233").unwrap();
        storage.put(b"2", b"2333").unwrap();
        storage.put(b"3", b"23333").unwrap();
        storage
            .force_freeze_memtable(&storage.state_lock.lock())
            .unwrap();
        storage.delete(b"1").unwrap();
        storage.delete(b"2").unwrap();
        storage.put(b"3", b"2333").unwrap();
        storage.put(b"4", b"23333").unwrap();
        storage
            .force_freeze_memtable(&storage.state_lock.lock())
            .unwrap();
        storage.put(b"1", b"233333").unwrap();
        storage.put(b"3", b"233333").unwrap();
        assert_eq!(storage.state.read().imm_memtables.len(), 2);
        assert_eq!(&storage.get(b"1").unwrap().unwrap()[..], b"233333");
        assert_eq!(&storage.get(b"2").unwrap(), &None);
        assert_eq!(&storage.get(b"3").unwrap().unwrap()[..], b"233333");
        assert_eq!(&storage.get(b"4").unwrap().unwrap()[..], b"23333");
    }
}
