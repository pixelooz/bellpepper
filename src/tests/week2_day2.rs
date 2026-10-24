#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use crate::{
        compact::{
            CompactionOptions,
            simple_leveled::{
                SimpleLeveledCompactionController, SimpleLeveledCompactionOptions,
                SimpleLeveledCompactionTask,
            },
        },
        lsm_options::LsmStorageOptions,
        lsm_storage::MiniLsm,
        tests::harness::{check_compaction_ratio, compaction_bench},
    };

    #[test]
    fn test_integration() {
        let dir = tempdir().unwrap();
        let storage = MiniLsm::open(
            &dir,
            LsmStorageOptions::default_for_week2_test(CompactionOptions::Simple(
                SimpleLeveledCompactionOptions {
                    l0_file_num_compaction_trigger: 2,
                    max_levels: 3,
                    size_ratio_percent: 200,
                },
            )),
        )
        .unwrap();

        compaction_bench(storage.clone());
        check_compaction_ratio(storage.clone());
    }

    #[test]
    fn test_l0_compaction_preserves_newer_ssts_in_order() {
        let options = SimpleLeveledCompactionOptions {
            l0_file_num_compaction_trigger: 2,
            max_levels: 3,
            size_ratio_percent: 200,
        };
        let controller = SimpleLeveledCompactionController::new(options.clone());
        let dir = tempdir().unwrap();
        let storage = MiniLsm::open(
            &dir,
            LsmStorageOptions::default_for_week2_test(CompactionOptions::Simple(options)),
        )
        .unwrap();
        let mut snapshot = storage.inner.state.read().as_ref().clone();
        snapshot.l0_ss_tables = vec![7, 6, 5, 4];
        let task = SimpleLeveledCompactionTask {
            upper_level: None,
            upper_level_sst_ids: vec![5, 4],
            lower_level: 1,
            lower_level_sst_ids: vec![],
            is_lower_level_bottom_level: false,
        };

        let (result, removed) = controller.apply_compaction_result(&snapshot, &task, &[]);
        assert_eq!(result.l0_ss_tables, vec![7, 6]);
        assert_eq!(removed, vec![5, 4]);
    }
}
