use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use bytes::Bytes;
use parking_lot::{Mutex, RwLock};

use crate::mem_table::MemTable;

#[derive(Debug, Clone)]
pub(crate) struct LsmStorageState {
    memtable: Arc<MemTable>,
    imm_memtables: Vec<Arc<MemTable>>,
}

impl LsmStorageState {
    pub fn create() -> Self {
        Self {
            memtable: Arc::new(MemTable::create(0)),
            imm_memtables: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct LsmStorageInner {
    pub(crate) state: Arc<RwLock<Arc<LsmStorageState>>>,
    pub(crate) state_lock: Mutex<()>,
    path: PathBuf,
}

impl LsmStorageInner {
    pub fn open(path: PathBuf) -> Self {
        let state = LsmStorageState::create();
        Self {
            state: Arc::new(RwLock::new(Arc::new(state))),
            state_lock: Mutex::new(()),
            path,
        }
    }

    pub fn put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        self.state.read().memtable.put(key, value)
    }

    pub fn get(&self, key: &[u8]) -> Result<Option<Bytes>> {
        let value = self.state.read().memtable.get(key);
        if value.as_ref().is_some_and(|x| x.is_empty()) {
            return Ok(None);
        }
        Ok(value)
    }

    pub fn delete(&self, key: &[u8]) -> Result<()> {
        self.put(key, b"")
    }
}
