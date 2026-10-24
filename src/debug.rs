use crate::lsm_storage::{LsmStorageInner, MiniLsm};

impl LsmStorageInner {
    pub fn dump_structure(&self) {
        let snapshot = self.state.read();
        if !snapshot.l0_ss_tables.is_empty() {
            println!(
                "L0 ({}): {:?}",
                snapshot.l0_ss_tables.len(),
                snapshot.l0_ss_tables,
            );
        }
        for (level, files) in &snapshot.levels {
            println!("L{level} ({}): {:?}", files.len(), files);
        }
    }
}

impl MiniLsm {
    pub fn dump_structure(&self) {
        self.inner.dump_structure()
    }
}
