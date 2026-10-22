#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bytes::Bytes;
    use tempfile::{TempDir, tempdir};

    use crate::{
        block::iterator::BlockIterator,
        iterators::StorageIterator,
        key::{KeySlice, KeyVec},
        table::{SsTable, builder::SsTableBuilder, iterator::SsTableIterator},
    };

    #[test]
    fn test_sst_build_single_key() {
        let mut builder = SsTableBuilder::new(16);
        builder.add(KeySlice::from_slice(b"233"), b"233333");
        let dir = tempdir().unwrap();
        builder.build_for_test(dir.path().join("1.sst")).unwrap();
    }

    #[test]
    fn test_sst_build_two_blocks() {
        let mut builder = SsTableBuilder::new(16);
        builder.add(KeySlice::from_slice(b"11"), b"11");
        builder.add(KeySlice::from_slice(b"22"), b"22");
        builder.add(KeySlice::from_slice(b"33"), b"11");
        builder.add(KeySlice::from_slice(b"44"), b"22");
        builder.add(KeySlice::from_slice(b"55"), b"11");
        builder.add(KeySlice::from_slice(b"66"), b"22");
        assert!(builder.block_metas.len() >= 2);
        let dir = tempdir().unwrap();
        builder.build_for_test(dir.path().join("1.sst")).unwrap();
    }

    fn key_of(idx: usize) -> KeyVec {
        KeyVec::from_vec(format!("key_{:03}", idx * 5).into_bytes())
    }

    fn value_of(idx: usize) -> Vec<u8> {
        format!("value_{:010}", idx).into_bytes()
    }

    fn num_of_keys() -> usize {
        100
    }

    fn generate_sst() -> (TempDir, SsTable) {
        let mut builder = SsTableBuilder::new(128);
        for idx in 0..num_of_keys() {
            let key = key_of(idx);
            let value = value_of(idx);
            builder.add(key.as_key_slice(), &value[..]);
        }
        let dir = tempdir().unwrap();
        let path = dir.path().join("1.sst");
        (dir, builder.build_for_test(path).unwrap())
    }

    #[test]
    fn test_sst_build_all() {
        let (_, sst) = generate_sst();
        assert_eq!(sst.first_key().as_key_slice(), key_of(0).as_key_slice());
        assert_eq!(
            sst.last_key().as_key_slice(),
            key_of(num_of_keys() - 1).as_key_slice()
        )
    }

    #[test]
    fn test_sst_block_metadata_matches_block_contents() {
        let (_dir, sst) = generate_sst();
        assert!(sst.num_of_blocks() > 1);

        for (block_idx, meta) in sst.blocks_meta.iter().enumerate() {
            let mut iter =
                BlockIterator::create_and_seek_to_first(sst.read_block(block_idx).unwrap());
            let first_key = Bytes::copy_from_slice(iter.key().raw_ref());
            let mut last_key = first_key.clone();
            while iter.is_valid() {
                last_key = Bytes::copy_from_slice(iter.key().raw_ref());
                iter.next();
            }

            assert_eq!(meta.first_key.raw_ref(), first_key.as_ref());
            assert_eq!(meta.last_key.raw_ref(), last_key.as_ref());
        }
    }

    #[test]
    fn test_sst_decode() {
        let (_dir, sst) = generate_sst();
        let meta = sst.blocks_meta.clone();
        let new_sst = SsTable::open_for_test(sst.file_object).unwrap();
        assert_eq!(new_sst.blocks_meta, meta);
        assert_eq!(new_sst.first_key().raw_ref(), key_of(0).raw_ref());
        assert_eq!(
            new_sst.last_key().raw_ref(),
            key_of(num_of_keys() - 1).raw_ref()
        );
    }

    fn as_bytes(x: &[u8]) -> Bytes {
        Bytes::copy_from_slice(x)
    }

    #[test]
    fn test_sst_iterator() {
        let (_dir, sst) = generate_sst();
        let sst = Arc::new(sst);
        let mut iter = SsTableIterator::create_and_seek_to_first(sst).unwrap();
        for _ in 0..5 {
            for i in 0..num_of_keys() {
                let key = iter.key();
                let value = iter.value();
                assert_eq!(
                    key.raw_ref(),
                    key_of(i).raw_ref(),
                    "expected key: {:?}, actual key: {:?}",
                    as_bytes(key_of(i).raw_ref()),
                    as_bytes(key.raw_ref())
                );
                assert_eq!(
                    value,
                    value_of(i),
                    "expected value: {:?}, actual value: {:?}",
                    as_bytes(&value_of(i)),
                    as_bytes(value)
                );
                iter.next().unwrap();
            }
            iter.seek_to_first().unwrap();
        }
    }

    #[test]
    fn test_sst_seek_key() {
        let (_dir, sst) = generate_sst();
        let sst = Arc::new(sst);
        let mut iter =
            SsTableIterator::create_and_seek_to_key(sst, key_of(0).as_key_slice()).unwrap();
        for offset in 1..=5 {
            for i in 0..num_of_keys() {
                let key = iter.key();
                let value = iter.value();
                assert_eq!(
                    key.raw_ref(),
                    key_of(i).raw_ref(),
                    "expected key: {:?}, actual key: {:?}",
                    as_bytes(key_of(i).raw_ref()),
                    as_bytes(key.raw_ref())
                );
                assert_eq!(
                    value,
                    value_of(i),
                    "expected value: {:?}, actual value: {:?}",
                    as_bytes(&value_of(i)),
                    as_bytes(value)
                );
                iter.seek_to_key(KeySlice::from_slice(
                    &format!("key_{:03}", i * 5 + offset).into_bytes(),
                ))
                .unwrap();
            }
            iter.seek_to_key(KeySlice::from_slice(b"k")).unwrap();
        }
    }
}
