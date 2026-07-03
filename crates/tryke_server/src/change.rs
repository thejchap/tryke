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
        // Three filters on the way in. All three are no-ops for the
        // FS watcher caller (it filters upstream); they're a
        // correctness fix for the `did_change` caller, which takes
        // client-supplied paths and must defend itself.
        //   1. In-root. Stops `/etc/passwd` and similar from reaching
        //      discovery, where `path_to_module` would otherwise turn
        //      it into a phantom module entry.
        //   2. `.py` extension. `rediscover_changed` already silently
        //      ignores non-Python files, but `affected_modules` would
        //      turn `README.md` into a bogus module name, spuriously
        //      marking the pool dirty (→ unnecessary worker restart).
        //   3. Not excluded by gitignore / .ignore / [tool.tryke]
        //      exclude. `.venv/`, vendored deps, etc. — same matcher
        //      the FS watcher applies at watcher.rs:43-47. Without
        //      this a client could ingest `.venv/whatever.py` into
        //      discovery (and any subsequent `run` would try to
        //      import + execute it).
        let paths: Vec<PathBuf> = guard
            .filter_in_root(paths)
            .into_iter()
            .filter(|p| p.extension().is_some_and(|ext| ext == "py"))
            .filter(|p| !guard.is_excluded(p))
            .collect();
        if paths.is_empty() {
            debug!("apply_change: no eligible paths — skipping");
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
