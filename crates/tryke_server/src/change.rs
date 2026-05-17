//! Shared "apply a file-change batch" helper used by both the FS watcher
//! task (`server.rs`) and the `did_change` RPC handler (`handler.rs`).
//!
//! Centralises the steps that must happen for every accepted change set:
//!   1. ask `Discoverer::affected_modules` which Python modules the change
//!      reaches
//!   2. `rediscover_changed` to refresh the discovery cache for those files
//!   3. compute `tests_for_changed` for the broadcast payload
//!   4. mark the worker pool `dirty` (next `run` will drain → restart)
//!   5. broadcast `discover_complete` so connected clients can refresh
//!
//! Keeping these steps in one place guarantees the FS path and the RPC
//! path stay semantically identical — important because the RPC path is
//! what closes the race for cooperating clients, and the FS path is what
//! covers everyone else.

use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use bytes::Bytes;
use log::debug;
use tokio::sync::{Mutex, broadcast};
use tryke_discovery::Discoverer;

use crate::protocol::{DiscoverCompleteParams, Notification};

/// Apply an accepted file-change batch to the shared discovery cache and
/// signal that the worker pool needs to be restarted before the next
/// dispatch.
///
/// Idempotent: calling twice with the same paths (e.g. once from
/// `did_change`, once from the FS watcher a few ms later) is harmless —
/// `rediscover_changed` re-reads the same files, the atomic store is a
/// no-op when `dirty` is already `true`, and the second
/// `discover_complete` broadcast is a duplicate that client-side
/// suppression can drop.
pub(crate) async fn apply_change(
    disc: &Mutex<Discoverer>,
    bcast_tx: &broadcast::Sender<Bytes>,
    dirty: &AtomicBool,
    paths: &[PathBuf],
) {
    if paths.is_empty() {
        return;
    }

    let (modules, tests) = {
        let mut guard = disc.lock().await;
        // Drop anything outside the project root before doing any
        // discovery work. The FS watcher already emits only in-tree
        // paths, so this is a no-op for that caller; for `did_change`
        // (which accepts client-supplied paths over TCP) it stops a
        // local process from polluting the import graph with arbitrary
        // files. `Discoverer::filter_in_root` does the canonicalisation
        // both sides need.
        let paths = guard.filter_in_root(paths);
        if paths.is_empty() {
            debug!("apply_change: all paths outside project root — skipping");
            return;
        }
        let modules = guard.affected_modules(&paths);
        guard.rediscover_changed(&paths);
        let tests = guard.tests_for_changed(&paths);
        // Set `dirty` *inside* the disc.lock critical section. Paired
        // with `execute_run`'s drain (which also happens inside its
        // disc.lock guard), this serialises the "discovery is updated
        // AND workers need restart" transition: a concurrent run can't
        // read tests refreshed by this apply_change while observing
        // dirty=false. Without this ordering, a run could acquire
        // disc.lock between our release and the dirty.store, snapshot
        // the new tests, and dispatch on workers whose `sys.modules`
        // still reflect the *previous* discovery — exactly the race
        // the in-band `did_change` is here to close.
        if !modules.is_empty() {
            dirty.store(true, Ordering::Release);
        }
        (modules, tests)
    };

    if modules.is_empty() {
        debug!("apply_change: no modules affected — skipping dirty mark");
    } else {
        debug!(
            "apply_change: marking pool dirty for {} module(s): {}",
            modules.len(),
            modules.join(", ")
        );
    }

    debug!("apply_change: {} affected tests", tests.len());
    let notif = Notification {
        jsonrpc: "2.0".to_string(),
        method: "discover_complete".to_string(),
        params: DiscoverCompleteParams { tests },
    };
    if let Ok(mut bytes) = serde_json::to_vec(&notif) {
        bytes.push(b'\n');
        let _ = bcast_tx.send(Bytes::from(bytes));
    }
}
