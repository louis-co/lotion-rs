//! Debounced, off-main-thread persistence of [`AppState`].
//!
//! Window moves/resizes, focus changes and tab title updates can fire dozens of
//! times per second. Encrypting and writing the state file synchronously for
//! each of them (previously including a 100k-iteration PBKDF2 derivation) made
//! the UI thread stutter. Instead, callers hand a snapshot to [`StateStore`],
//! which coalesces bursts and writes the newest snapshot on a background thread.

use crate::state::{AppState, StateKey};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;

const DEBOUNCE: Duration = Duration::from_millis(500);

enum Msg {
    Save(Box<AppState>),
    Flush(Sender<()>),
}

pub struct StateStore {
    tx: Mutex<Option<Sender<Msg>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl StateStore {
    pub fn new(key: StateKey) -> Self {
        let (tx, rx) = mpsc::channel::<Msg>();
        let worker = std::thread::Builder::new()
            .name("lotion-state-writer".into())
            .spawn(move || {
                let write = |state: &AppState| {
                    if let Err(e) = state.save_with_key(&key) {
                        log::error!("Failed to persist AppState: {}", e);
                    }
                };
                let mut pending: Option<Box<AppState>> = None;
                loop {
                    let msg = if pending.is_some() {
                        match rx.recv_timeout(DEBOUNCE) {
                            Ok(msg) => msg,
                            Err(RecvTimeoutError::Timeout) => {
                                if let Some(state) = pending.take() {
                                    write(&state);
                                }
                                continue;
                            }
                            Err(RecvTimeoutError::Disconnected) => break,
                        }
                    } else {
                        match rx.recv() {
                            Ok(msg) => msg,
                            Err(_) => break,
                        }
                    };
                    match msg {
                        Msg::Save(state) => pending = Some(state),
                        Msg::Flush(ack) => {
                            if let Some(state) = pending.take() {
                                write(&state);
                            }
                            let _ = ack.send(());
                        }
                    }
                }
                if let Some(state) = pending.take() {
                    write(&state);
                }
            })
            .expect("failed to spawn state writer thread");

        Self {
            tx: Mutex::new(Some(tx)),
            worker: Mutex::new(Some(worker)),
        }
    }

    /// Queue a snapshot for writing. Cheap; never blocks on disk I/O.
    pub fn save(&self, state: &AppState) {
        if let Some(tx) = self.tx.lock().expect("StateStore lock poisoned").as_ref() {
            let mut snapshot = state.clone();
            snapshot.prune_orphan_tabs();
            let _ = tx.send(Msg::Save(Box::new(snapshot)));
        }
    }

    /// Write any pending snapshot now and wait for it to hit the disk.
    pub fn flush(&self) {
        let (ack_tx, ack_rx) = mpsc::channel();
        if let Some(tx) = self.tx.lock().expect("StateStore lock poisoned").as_ref() {
            if tx.send(Msg::Flush(ack_tx)).is_ok() {
                let _ = ack_rx.recv_timeout(Duration::from_secs(2));
            }
        }
    }

    /// Flush and stop the writer thread.
    pub fn shutdown(&self) {
        self.flush();
        self.tx.lock().expect("StateStore lock poisoned").take();
        if let Some(worker) = self.worker.lock().expect("StateStore lock poisoned").take() {
            let _ = worker.join();
        }
    }
}
