//! Raise the process's `RLIMIT_NOFILE` soft limit on startup.
//!
//! Modern systemd ships a soft default of 1024 and a hard default of
//! 524288 open files per service (and inherits the same shape to user
//! shells on most distributions). The convention systemd encourages —
//! and what Home Assistant OS 16 moved to in mid-2025 — is for each
//! application to **explicitly raise its own soft limit at startup**,
//! up to whatever hard limit it was launched under, rather than rely
//! on the OS to ship a huge soft default.[^1]
//!
//! Tryke spawns one Python subprocess per worker and each subprocess
//! accumulates file descriptors over its lifetime (test runs open
//! sockets, sqlite handles, log files, doctest stdio pipes, ...). On
//! macOS the inherited soft limit is famously 256, which large suites
//! (e.g. ~5k tests) exhaust well before they finish — and the failure
//! mode is opaque: a Python `OSError: [Errno 24] Too many open files`
//! buried inside a fixture. Raising the runner's own soft limit on
//! startup lifts that ceiling for every spawned worker (children
//! inherit our rlimit), so the FD-exhaustion failure mode disappears
//! without having to recycle worker processes.
//!
//! Behaviour:
//! - Unix: read the current `(soft, hard)` via `getrlimit(RLIMIT_NOFILE)`
//!   and call `setrlimit` to raise `soft` to `hard`. macOS has a
//!   kernel-side ceiling that is often *lower* than the reported hard
//!   limit (`kern.maxfilesperproc`); we don't try to detect that
//!   directly — instead the setrlimit call fails cleanly and we fall
//!   back to a conservative target (`OPEN_MAX_FALLBACK`) so the binary
//!   still benefits from a meaningful bump.
//! - Windows: no-op. The platform has no `RLIMIT_NOFILE`; the C
//!   runtime's `_setmaxstdio` ceiling (2048 by default) is the closest
//!   analogue but it does not affect socket / pipe FDs.
//!
//! Errors are non-fatal: this module returns the `io::Result` so the
//! caller can choose how to surface a failure (in `main` we log it at
//! `warn` and proceed). The user can still override the limit
//! manually via `ulimit -n` before launching tryke if a sandbox or
//! cgroup pinned it.
//!
//! [^1]: <https://developers.home-assistant.io/blog/2025/07/14/home-assistant-os-16-open-file-limit/>

/// Conservative fallback target for `RLIMIT_NOFILE` when the kernel
/// rejects raising soft to the reported hard limit. macOS in
/// particular caps per-process FDs below `RLIM_INFINITY` via
/// `kern.maxfilesperproc` (default 24 576 on recent releases), so an
/// unconditional `soft = hard` setrlimit returns `EINVAL`. The value
/// chosen here is high enough to dwarf the 256-FD macOS soft default
/// that bites real test suites, while staying below historic macOS
/// per-process ceilings so the fallback itself succeeds.
#[cfg(unix)]
const OPEN_MAX_FALLBACK: libc::rlim_t = 10_240;

/// Outcome of [`raise`]. Carries the before/after soft limit so the
/// caller can log meaningful telemetry without re-querying the
/// kernel. `Skipped` covers platforms (windows) where the operation
/// has no analogue, and the case where the inherited soft limit
/// already saturates the hard ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaiseOutcome {
    /// The soft limit was raised from `from` to `to`. Both are FD
    /// counts (matching the units reported by `ulimit -n`).
    Raised { from: u64, to: u64 },
    /// No change applied. Either the platform has no `RLIMIT_NOFILE`
    /// or the soft limit already matched (or exceeded) the hard
    /// limit. `current` carries the existing soft limit for logging.
    Skipped { current: u64 },
}

#[cfg(unix)]
pub fn raise() -> std::io::Result<RaiseOutcome> {
    use std::io::Error;

    // SAFETY: `rlimit` is a POD struct populated by the kernel and
    // `getrlimit` only touches the bytes we pass it. The address is
    // stack-local and lives for the duration of the call.
    let mut rlim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    let rc = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut rlim) };
    if rc != 0 {
        return Err(Error::last_os_error());
    }

    // Already at or above the hard limit — leave it. We never *lower*
    // the soft limit (that would punish users who deliberately raised
    // it via `ulimit -n` before launching) and we don't try to push
    // the hard limit (that requires CAP_SYS_RESOURCE / root on Linux
    // and adds a failure mode without a clear benefit).
    if rlim.rlim_cur >= rlim.rlim_max {
        #[expect(
            clippy::useless_conversion,
            reason = "rlim_t is u32 on some targets, u64 on others; cast unifies the wire type"
        )]
        return Ok(RaiseOutcome::Skipped {
            current: u64::from(rlim.rlim_cur),
        });
    }

    let original_soft = rlim.rlim_cur;
    let target = rlim.rlim_max;
    rlim.rlim_cur = target;
    // SAFETY: `rlim` is a fully initialised POD on our stack; the
    // kernel only reads from it.
    let rc = unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raw const rlim) };
    if rc == 0 {
        #[expect(
            clippy::useless_conversion,
            reason = "rlim_t is u32 on some targets, u64 on others; cast unifies the wire type"
        )]
        return Ok(RaiseOutcome::Raised {
            from: u64::from(original_soft),
            to: u64::from(target),
        });
    }

    // setrlimit failed at the hard ceiling — macOS's
    // `kern.maxfilesperproc` is the usual culprit. Retry with a
    // conservative fallback: the smaller of the reported hard limit
    // and `OPEN_MAX_FALLBACK`. If the fallback target is still ≤ the
    // existing soft limit (very unlikely), report `Skipped`.
    let fallback_target = rlim.rlim_max.min(OPEN_MAX_FALLBACK);
    if fallback_target <= original_soft {
        #[expect(
            clippy::useless_conversion,
            reason = "rlim_t is u32 on some targets, u64 on others; cast unifies the wire type"
        )]
        return Ok(RaiseOutcome::Skipped {
            current: u64::from(original_soft),
        });
    }
    rlim.rlim_cur = fallback_target;
    // SAFETY: see above.
    let rc = unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raw const rlim) };
    if rc != 0 {
        return Err(Error::last_os_error());
    }
    #[expect(
        clippy::useless_conversion,
        reason = "rlim_t is u32 on some targets, u64 on others; cast unifies the wire type"
    )]
    Ok(RaiseOutcome::Raised {
        from: u64::from(original_soft),
        to: u64::from(fallback_target),
    })
}

#[cfg(not(unix))]
pub fn raise() -> std::io::Result<RaiseOutcome> {
    // No `RLIMIT_NOFILE` analogue on windows. The C runtime's
    // `_setmaxstdio` only governs stdio streams (not sockets or
    // pipes), so raising it would not help worker-spawned children.
    Ok(RaiseOutcome::Skipped { current: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On Unix the call must report either a successful raise or an
    /// explicit skip — never an error under normal CI conditions.
    /// Asserting on the exact `to` value would be flaky (it depends
    /// on the host's hard limit) so we only check the shape.
    #[cfg(unix)]
    #[test]
    fn raise_returns_meaningful_outcome_on_unix() {
        let outcome = raise().expect("raise should not fail under default CI rlimits");
        match outcome {
            RaiseOutcome::Raised { from, to } => {
                assert!(to >= from, "raise must be monotonic: from={from} to={to}");
            }
            RaiseOutcome::Skipped { current } => {
                assert!(
                    current > 0,
                    "skipped outcomes must report the current limit"
                );
            }
        }
    }

    /// Idempotency: calling `raise` twice in a row must not lower the
    /// soft limit or error. The second call typically returns
    /// `Skipped` (already at hard).
    #[cfg(unix)]
    #[test]
    fn raise_is_idempotent() {
        let first = raise().expect("first raise");
        let second = raise().expect("second raise");
        let first_to = match first {
            RaiseOutcome::Raised { to, .. } | RaiseOutcome::Skipped { current: to } => to,
        };
        let second_to = match second {
            RaiseOutcome::Raised { to, .. } | RaiseOutcome::Skipped { current: to } => to,
        };
        assert!(
            second_to >= first_to,
            "soft limit must not drop across calls: first={first_to} second={second_to}",
        );
    }
}
