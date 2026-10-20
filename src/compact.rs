use std::{
    sync::Arc,
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::Result;

use crate::lsm_storage::LsmStorageInner;

impl LsmStorageInner {
    /// Evaluates the condition of flushing memtables which if true flushes the oldest
    /// memtable to the `l0_ss_table`.
    fn trigger_flush(&self) -> Result<()> {
        let should_flush = {
            let state_guard = self.state.read();
            state_guard.imm_memtables.len() >= self.options.num_memtable_limit
        };
        if should_flush {
            self.force_flush_next_imm_memtable()?;
        }
        Ok(())
    }

    /// Spawns a background thread triggering a flush every 50 milliseconds or returns
    /// early if receiver is closed.
    pub(crate) fn spawn_flush_thread(
        self: &Arc<Self>,
        rx: crossbeam_channel::Receiver<()>,
    ) -> Result<Option<JoinHandle<()>>> {
        let inner = self.clone();

        let handle = thread::spawn(move || {
            let ticker = crossbeam_channel::tick(Duration::from_millis(50));
            loop {
                crossbeam_channel::select! {
                    recv(ticker) -> _ => if let Err(err) = inner.trigger_flush() {
                        eprintln!("flush failed: {}", err);
                    },
                    recv(rx) -> _ => return
                }
            }
        });
        Ok(Some(handle))
    }
}
