// Copyright (c) 2022-2026 Alex Chi Z
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::lsm_storage::LsmStorageState;

#[derive(Debug, Serialize, Deserialize)]
pub struct LeveledCompactionTask {
    // if upper_level is `None`, then it is L0 compaction
    pub upper_level: Option<usize>,
    pub upper_level_sst_ids: Vec<usize>,
    pub lower_level: usize,
    pub lower_level_sst_ids: Vec<usize>,
    pub is_lower_level_bottom_level: bool,
}

#[derive(Debug, Clone)]
pub struct LeveledCompOpt {
    pub level_size_multiplier: usize,
    pub level0_file_num_compaction_trigger: usize,
    pub max_levels: usize,
    pub base_level_size_mb: usize,
}

pub struct LeveledCc {
    options: LeveledCompOpt,
}

impl LeveledCc {
    pub fn new(options: LeveledCompOpt) -> Self {
        Self { options }
    }

    fn find_overlapping_ssts(
        &self,
        snapshot: &LsmStorageState,
        sst_ids: &[usize],
        in_level: usize,
    ) -> Vec<usize> {
        let begin_key = sst_ids
            .iter()
            .map(|id| snapshot.ss_tables[id].first_key())
            .min()
            .unwrap();
        let end_key = sst_ids
            .iter()
            .map(|id| snapshot.ss_tables[id].last_key())
            .max()
            .unwrap();
        let mut overlap_ssts = Vec::new();
        for sst_id in &snapshot.levels[in_level - 1].1 {
            let sst = &snapshot.ss_tables[sst_id];
            if !(sst.first_key() > end_key || sst.last_key() < begin_key) {
                overlap_ssts.push(*sst_id);
            }
        }
        overlap_ssts
    }

    pub fn generate_compaction_task(
        &self,
        snapshot: &LsmStorageState,
    ) -> Option<LeveledCompactionTask> {
        let base_level_size = self.options.base_level_size_mb * 1024 * 1024;
        let max_level = self.options.max_levels;
        // The sizes are stored on 1 based indexing, not zero, so arr[0] will be zero for both.
        let mut target_level_size = vec![0usize; max_level + 1];
        let mut real_level_size = Vec::with_capacity(max_level + 1);
        for (_, sst_ids) in &snapshot.levels {
            let real_size = sst_ids
                .iter()
                .map(|id| snapshot.ss_tables[id].table_size())
                .sum::<u64>();
            real_level_size.push(usize::try_from(real_size).unwrap());
        }
        target_level_size[max_level - 1] = real_level_size[max_level - 1].max(base_level_size);
        let mut base_level = max_level;
        for level in (1..max_level).rev() {
            let next_level_target = target_level_size[level + 1];
            if next_level_target > base_level_size {
                target_level_size[level] = next_level_target / self.options.level_size_multiplier;
                if target_level_size[level] > 0 {
                    base_level = level;
                }
            }
        }
        if snapshot.l0_ss_tables.len() >= self.options.level0_file_num_compaction_trigger {
            return Some(LeveledCompactionTask {
                upper_level: None,
                upper_level_sst_ids: snapshot.l0_ss_tables.clone(),
                lower_level: base_level,
                lower_level_sst_ids: self.find_overlapping_ssts(
                    snapshot,
                    &snapshot.l0_ss_tables,
                    base_level,
                ),
                is_lower_level_bottom_level: base_level == max_level,
            });
        }
        let mut priorities = Vec::new();
        for level in 1..max_level {
            let prio = real_level_size[level] as f64 / target_level_size[level] as f64;
            if prio > 1.0 {
                priorities.push((prio, level));
            }
        }
        priorities.sort_by(|a, b| a.partial_cmp(b).unwrap().reverse());
        let (_, level) = *priorities.first()?;
        let selected_sst = *snapshot.levels[level - 1].1.iter().min().unwrap();
        Some(LeveledCompactionTask {
            upper_level: Some(level),
            upper_level_sst_ids: vec![selected_sst],
            lower_level: level + 1,
            lower_level_sst_ids: self.find_overlapping_ssts(snapshot, &[selected_sst], level + 1),
            is_lower_level_bottom_level: level + 1 == max_level,
        })
    }

    pub fn apply_compaction_result(
        &self,
        snapshot: &LsmStorageState,
        task: &LeveledCompactionTask,
        output: &[usize],
        in_recovery: bool,
    ) -> (LsmStorageState, Vec<usize>) {
        let mut snapshot = snapshot.clone();
        let mut upper_level_sst_id_set = task
            .upper_level_sst_ids
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        let mut lower_level_sst_id_set = task
            .lower_level_sst_ids
            .iter()
            .copied()
            .collect::<HashSet<_>>();

        let mut files_to_remove = Vec::<usize>::new();
        files_to_remove.extend(&task.lower_level_sst_ids);
        files_to_remove.extend(&task.upper_level_sst_ids);

        match task.upper_level {
            Some(upper_level) => {
                snapshot.levels[upper_level - 1]
                    .1
                    .retain(|id| !upper_level_sst_id_set.remove(id));
                assert!(upper_level_sst_id_set.is_empty());
            }
            None => {
                snapshot
                    .l0_ss_tables
                    .retain(|id| !upper_level_sst_id_set.remove(id));
                assert!(upper_level_sst_id_set.is_empty());
            }
        };
        let lower_level_sst_ids = &mut snapshot.levels[task.lower_level - 1].1;
        lower_level_sst_ids.retain(|id| !lower_level_sst_id_set.remove(id));
        assert!(lower_level_sst_id_set.is_empty());
        lower_level_sst_ids.extend(output);
        if !in_recovery {
            lower_level_sst_ids.sort_by(|a, b| {
                snapshot.ss_tables[a]
                    .first_key()
                    .cmp(snapshot.ss_tables[b].first_key())
            });
        }
        (snapshot, files_to_remove)
    }
}
