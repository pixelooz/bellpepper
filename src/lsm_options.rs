use crate::compact::CompactionOptions;

/// Options for initializing the Lsm storage engine.
#[derive(Debug, Clone)]
pub struct LsmStorageOptions {
    /// Block size in bytes.
    pub block_size: usize,

    /// SST size in bytes, and also the approximate size of memtables.
    pub target_sst_size: usize,

    /// Number of memtables allowed in memory before flushing them to L0.
    pub num_memtable_limit: usize,

    pub compaction_options: CompactionOptions,
}

impl LsmStorageOptions {
    pub fn default_for_week1_test() -> Self {
        Self {
            block_size: 4096,
            target_sst_size: 2 << 20,
            num_memtable_limit: 50,
            compaction_options: CompactionOptions::NoCompaction,
        }
    }

    pub fn default_for_week1_day6_test() -> Self {
        Self {
            block_size: 4096,
            target_sst_size: 2 << 20,
            num_memtable_limit: 2,
            compaction_options: CompactionOptions::NoCompaction,
        }
    }
    //
    //     pub fn default_for_week2_test(compaction_options: CompactionOptions) -> Self {
    //         Self {
    //             block_size: 4096,
    //             target_sst_size: 1 << 20, // 1MB
    //             num_memtable_limit: 2,
    //         }
    //     }
}
