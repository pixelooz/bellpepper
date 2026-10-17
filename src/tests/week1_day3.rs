#[cfg(test)]
mod tests {
    use crate::{
        block::{Block, builder::BlockBuilder},
        key::{KeySlice, KeyVec},
    };

    fn key_of(idx: usize) -> KeyVec {
        KeyVec::from_vec(format!("key_{:03}", idx * 5).into_bytes())
    }

    fn value_of(idx: usize) -> Vec<u8> {
        format!("value_{:010}", idx).into_bytes()
    }

    fn num_of_keys() -> usize {
        100
    }

    fn generate_block() -> Block {
        let mut builder = BlockBuilder::new(10000);
        for idx in 0..num_of_keys() {
            let key = key_of(idx);
            let value = value_of(idx);
            assert!(builder.add(key.as_key_slice(), &value[..]));
        }
        builder.build()
    }

    #[test]
    fn test_block_build_single_key() {
        let mut builder = BlockBuilder::new(16);
        assert!(builder.add(KeySlice::from_slice(b"233"), b"233333"));
        builder.build();
    }

    #[test]
    fn test_block_build_full() {
        let mut builder = BlockBuilder::new(16);
        assert!(builder.add(KeySlice::from_slice(b"11"), b"11"));
        assert!(!builder.add(KeySlice::from_slice(b"22"), b"22"));
        builder.build();
    }

    #[test]
    fn test_block_build_large_1() {
        let mut builder = BlockBuilder::new(16);
        assert!(builder.add(KeySlice::from_slice(b"11"), &b"1".repeat(100)));
        builder.build();
    }

    #[test]
    fn test_block_encode() {
        let block = generate_block();
        block.encode();
    }

    #[test]
    fn test_block_decode() {
        let block = generate_block();
        let encoded = block.encode();
        let decoded_block = Block::decode(&encoded);
        assert_eq!(block.offsets, decoded_block.offsets);
        assert_eq!(block.data, decoded_block.data);
    }
}
