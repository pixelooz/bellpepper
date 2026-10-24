use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::lsm_storage::LsmStorageState;

#[derive(Debug, Serialize, Deserialize)]
pub struct SimpleLeveledCompactionTask {
    // if upper_level is `None`, then it is L0 compaction
    pub upper_level: Option<usize>,
    pub upper_level_sst_ids: Vec<usize>,
    pub lower_level: usize,
    pub lower_level_sst_ids: Vec<usize>,
    pub is_lower_level_bottom_level: bool,
}

#[derive(Debug, Clone)]
pub struct SimpleLeveledCompactionOptions {
    pub size_ratio_percent: usize,
    pub l0_file_num_compaction_trigger: usize,
    pub max_levels: usize,
}

pub struct SimpleLeveledCompactionController {
    options: SimpleLeveledCompactionOptions,
}

impl SimpleLeveledCompactionController {
    pub fn new(options: SimpleLeveledCompactionOptions) -> Self {
        Self { options }
    }

    /// Generates a compaction task. Evaluates L0 file count and level size ratios to schedule
    /// the next compaction task.
    ///
    /// Returns `None` if no compaction needs to be scheduled. The order of SSTs in the
    /// compaction task id vector matters.
    pub fn generate_compaction_task(
        &self,
        snapshot: &LsmStorageState,
    ) -> Option<SimpleLeveledCompactionTask> {
        if snapshot.l0_ss_tables.len() >= self.options.l0_file_num_compaction_trigger {
            return Some(SimpleLeveledCompactionTask {
                upper_level: None,
                upper_level_sst_ids: snapshot.l0_ss_tables.clone(),
                lower_level: 1,
                lower_level_sst_ids: snapshot.levels[0].1.clone(),
                is_lower_level_bottom_level: self.options.max_levels == 1,
            });
        }
        let target_ratio = (self.options.size_ratio_percent as f64) / 100.0;
        for upper_level in 1..self.options.max_levels {
            let lower_level = upper_level + 1;
            let upper_ssts = snapshot.levels[upper_level - 1].1.clone();
            if upper_ssts.is_empty() {
                continue;
            }
            let lower_ssts = snapshot.levels[lower_level - 1].1.clone();
            let current_ratio = (lower_ssts.len() as f64) / (upper_ssts.len() as f64);
            if current_ratio < target_ratio {
                return Some(SimpleLeveledCompactionTask {
                    upper_level: Some(upper_level),
                    upper_level_sst_ids: upper_ssts,
                    lower_level,
                    lower_level_sst_ids: lower_ssts,
                    is_lower_level_bottom_level: lower_level == self.options.max_levels,
                });
            }
        }
        None
    }

    /// Applied the completed compaction task to a state snapshot and returns the obsolete SST IDs.
    ///
    /// The compactor will call this function with the compaction task and the list of SST
    /// ids generated. This function applies the result and generates a new LSM state.
    /// The functions should only change `l0_ss_tables` and `levels` without changing memtables
    /// and `ss_tables` hash map. Though there should only be one thread running compaction
    /// jobs, you should think about the case where an L0 SST gets flushed while the compactor
    /// generates new SSTs, and with that in mind, you should do some sanity checks in your
    /// implementation.
    pub fn apply_compaction_result(
        &self,
        snapshot: &LsmStorageState,
        task: &SimpleLeveledCompactionTask,
        output: &[usize],
    ) -> (LsmStorageState, Vec<usize>) {
        let mut files_to_remove =
            Vec::with_capacity(task.upper_level_sst_ids.len() + task.lower_level_sst_ids.len());

        let mut snapshot = snapshot.clone();
        match task.upper_level {
            Some(upper_level) => {
                let upper_ssts = &mut snapshot.levels[upper_level - 1].1;
                assert_eq!(
                    *upper_ssts, task.upper_level_sst_ids,
                    "upper level L{upper_level} SSTs changed during compaction"
                );
                files_to_remove.extend_from_slice(upper_ssts);
                upper_ssts.clear();
            }
            None => {
                files_to_remove.extend_from_slice(&task.upper_level_sst_ids);
                let mut compacted_l0: HashSet<usize> =
                    task.upper_level_sst_ids.iter().copied().collect();
                snapshot.l0_ss_tables.retain(|id| !compacted_l0.remove(id));
                assert!(
                    compacted_l0.is_empty(),
                    "some compacted L0 SSTs were missing from snapshot.l0_ss_tables"
                );
            }
        }
        let lower_ssts = &mut snapshot.levels[0].1;
        assert_eq!(
            *lower_ssts, task.lower_level_sst_ids,
            "upper level L{} SSTs changed during compaction",
            task.lower_level
        );
        files_to_remove.extend_from_slice(lower_ssts);
        *lower_ssts = output.to_vec();
        (snapshot, files_to_remove)
    }
}
