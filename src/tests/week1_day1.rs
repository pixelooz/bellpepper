use crate::mem_table::MemTable;

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
