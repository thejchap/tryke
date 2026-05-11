use std::collections::VecDeque;
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use log::{debug, trace};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tryke_types::{Assertion, ExpectedAssertion, TestItem, TestOutcome, TestResult};

use crate::protocol::{
    AssertionWire, FinalizeHooksParams, RegisterHooksParams, RpcRequest, RpcResponse,
    RunDoctestParams, RunTestParams, RunTestResponseWire, RunTestResultWire, WorkerHealthWire,
};

/// Cap on retained worker-stderr bytes. Beyond this we keep the most recent
/// bytes and drop older ones — enough for diagnostic context on failures
/// without unbounded memory growth on workers that spew warnings.
const STDERR_RETAIN_BYTES: usize = 1 << 20; // 1 MiB

/// Latest worker-reported resource snapshot. Each field is `Option`
/// because some signals are platform-conditional (see
/// [`WorkerHealthWire`]); a missing reading just means the matching
/// limit cannot fire for this worker.
#[derive(Debug, Default, Clone, Copy)]
pub struct WorkerHealth {
    pub rss_bytes: Option<u64>,
    pub open_fds: Option<u64>,
}

impl From<WorkerHealthWire> for WorkerHealth {
    fn from(w: WorkerHealthWire) -> Self {
        Self {
            rss_bytes: w.rss_bytes,
            open_fds: w.open_fds,
        }
    }
}

/// Why a worker is being recycled. Carried in debug logs so post-mortem
/// triage can tell apart memory leaks, FD pressure, and slow drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecycleReason {
    /// Resident set size crossed the configured ceiling.
    MemoryBytes(u64),
    /// Open file descriptor count crossed the configured ceiling.
    OpenFds(u64),
    /// Worker has been alive longer than the configured ceiling.
    Age(Duration),
}

impl fmt::Display for RecycleReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MemoryBytes(b) => write!(f, "memory={b} bytes"),
            Self::OpenFds(n) => write!(f, "open_fds={n}"),
            Self::Age(d) => write!(f, "age={:.1}s", d.as_secs_f64()),
        }
    }
}

/// Soft ceilings on a worker's resource footprint. Each is `Option` so
/// callers can opt out of a signal entirely (e.g. tests that exercise
/// only one dimension). `None` on a field means "never recycle on this
/// signal." The defaults are tuned for real test suites — see
/// [`WorkerLimits::default`].
#[derive(Debug, Clone, Copy)]
pub struct WorkerLimits {
    pub max_rss_bytes: Option<u64>,
    pub max_open_fds: Option<u64>,
    pub max_age: Option<Duration>,
}

impl WorkerLimits {
    /// Disable every soft limit. Convenient for tests that only want to
    /// exercise one signal at a time.
    #[must_use]
    pub fn unlimited() -> Self {
        Self {
            max_rss_bytes: None,
            max_open_fds: None,
            max_age: None,
        }
    }
}

impl Default for WorkerLimits {
    fn default() -> Self {
        Self {
            // 1 GiB RSS — comfortable headroom for suites that pull in
            // heavy native deps (numpy, ssl, sqlite) on first import,
            // while still bounding slow leaks across long runs.
            max_rss_bytes: Some(1 << 30),
            // Below the macOS 256-FD per-process soft limit, leaving
            // headroom for the python interpreter's own baseline (~30
            // FDs on a fresh process) plus the runner's stdio pipes.
            max_open_fds: Some(200),
            // 10 minutes of wall time. Long suites still finish per
            // worker without churning; pathologically slow leaks that
            // never trip the memory/FD cap still get bounded.
            max_age: Some(Duration::from_secs(600)),
        }
    }
}

/// Pure recycle-decision helper, factored out of [`WorkerProcess`] so
/// it is unit-testable without spawning a subprocess. Signals are
/// checked in priority order: memory > FDs > age, so the strongest
/// available reason is what gets reported.
pub(crate) fn evaluate_recycle(
    health: WorkerHealth,
    age: Duration,
    limits: WorkerLimits,
) -> Option<RecycleReason> {
    if let (Some(cap), Some(rss)) = (limits.max_rss_bytes, health.rss_bytes)
        && rss >= cap
    {
        return Some(RecycleReason::MemoryBytes(rss));
    }
    if let (Some(cap), Some(fds)) = (limits.max_open_fds, health.open_fds)
        && fds >= cap
    {
        return Some(RecycleReason::OpenFds(fds));
    }
    if let Some(cap) = limits.max_age
        && age >= cap
    {
        return Some(RecycleReason::Age(age));
    }
    None
}

pub struct WorkerProcess {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    /// Continuously-drained worker stderr. A background task reads the
    /// child's stderr pipe into this buffer so the pipe never fills and
    /// the worker can't block on a stderr write mid-RPC.
    stderr_buf: Arc<Mutex<VecDeque<u8>>>,
    next_id: u64,
    /// Wall-clock spawn time; drives `RecycleReason::Age`. Set once at
    /// spawn and read on every `should_recycle` check — never mutated.
    spawned_at: Instant,
    /// Latest worker-reported resource snapshot. Updated after every
    /// completed `run_test` / `run_doctest` reply. The default
    /// (`Option::None` on every field) is what a fresh worker reads as
    /// before its first reply lands, which matches "no signal, do not
    /// recycle on it."
    latest_health: WorkerHealth,
    /// Soft ceilings consulted by [`Self::should_recycle`]. Owned by
    /// the worker (rather than passed in on each check) so the recycle
    /// decision is colocated with the data it depends on.
    limits: WorkerLimits,
}

impl WorkerProcess {
    /// Spawn a fresh worker process.
    ///
    /// `log_level` is forwarded as `TRYKE_LOG=<level>` on the child env so
    /// the python worker's `_configure_logging_from_env` lights up at the
    /// same level as the rust process. Pass `LevelFilter::Off` to keep the
    /// worker silent (no env var set), preserving the pre-existing
    /// "no chatter unless asked" default.
    #[expect(clippy::missing_errors_doc)]
    pub fn spawn(
        python_bin: &str,
        python_path: &[&Path],
        root: &Path,
        log_level: log::LevelFilter,
        limits: WorkerLimits,
    ) -> Result<Self> {
        debug!("spawning worker: {python_bin} -m tryke.worker (log={log_level})");
        let pythonpath = build_pythonpath(python_path);
        let mut command = Command::new(python_bin);
        command
            .args(["-m", "tryke.worker"])
            .env("PYTHONPATH", &pythonpath)
            .current_dir(root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Some(value) = worker_log_env_value(log_level) {
            command.env("TRYKE_LOG", value);
        }
        let mut child = command.spawn()?;
        let stdin = BufWriter::new(child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?);
        let stdout = BufReader::new(child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?);
        let stderr = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;
        debug!("worker spawned (pid {:?})", child.id());

        // The worker can write to stderr at any time (asyncio default
        // exception handler, library warnings, etc.). The kernel pipe
        // buffer defaults to 64 KiB on Linux, so without an active
        // reader the worker's next stderr write blocks once the pipe
        // fills — and the worker stops responding to RPCs, surfacing
        // as "tryke hangs at finalize_hooks". Spawn a drainer that
        // keeps the pipe empty for the worker's lifetime.
        let stderr_buf = Arc::new(Mutex::new(VecDeque::<u8>::new()));
        if let Err(err) = spawn_stderr_drainer(stderr, Arc::clone(&stderr_buf)) {
            if let Err(kill_err) = child.start_kill() {
                debug!(
                    "failed to kill worker after stderr drainer setup error (pid {:?}): {kill_err}",
                    child.id()
                );
            }
            return Err(err);
        }

        Ok(Self {
            child,
            stdin,
            stdout,
            stderr_buf,
            next_id: 1,
            spawned_at: Instant::now(),
            latest_health: WorkerHealth::default(),
            limits,
        })
    }

    /// Whether this worker has tripped any of its [`WorkerLimits`].
    /// Returns the first reason found (memory > FDs > age priority),
    /// or `None` if the worker is still within budget. Long-lived
    /// python interpreters accumulate module-level state across
    /// imports — logging handlers, sqlite/ssl objects, atexit
    /// callbacks — much of which holds resources (FDs, memory) that
    /// `del sys.modules[name]` cannot reclaim, because they are owned
    /// by objects living outside the module dict. Recycling on the
    /// reported snapshot bounds that growth; only process exit
    /// reliably frees module-level state in `CPython`.
    #[must_use]
    pub fn should_recycle(&self) -> Option<RecycleReason> {
        evaluate_recycle(self.latest_health, self.spawned_at.elapsed(), self.limits)
    }

    async fn call<R: for<'de> serde::Deserialize<'de>>(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<R> {
        let id = self.next_id;
        self.next_id += 1;
        let req = RpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };
        let mut line = serde_json::to_string(&req)?;
        line.push('\n');
        trace!("worker rpc -> {}", line.trim());
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        // Read lines from stdout, skipping non-JSON garbage that a native
        // library may have written to fd 1 during import (e.g. weasyprint via
        // cffi).  Collect leaked lines so we can surface them in errors.
        let mut leaked_stdout: Vec<String> = Vec::new();
        let resp: RpcResponse = loop {
            let mut resp_line = String::new();
            let n = self.stdout.read_line(&mut resp_line).await?;
            if n == 0 {
                trace!("worker rpc: stdout EOF");
                return Err(if leaked_stdout.is_empty() {
                    anyhow!("worker process closed stdout")
                } else {
                    anyhow!(
                        "worker process closed stdout after writing non-JSON output \
                         (a library may have written to stdout during import):\n{}",
                        leaked_stdout.join("")
                    )
                });
            }
            trace!("worker rpc <- {}", resp_line.trim());
            let trimmed = resp_line.trim();
            if !trimmed.is_empty()
                && let Ok(resp) = serde_json::from_str::<RpcResponse>(trimmed)
            {
                if !leaked_stdout.is_empty() {
                    trace!(
                        "worker rpc: skipped {} non-JSON line(s) on stdout",
                        leaked_stdout.len()
                    );
                }
                break resp;
            }
            leaked_stdout.push(resp_line);
            if leaked_stdout.len() >= 50 {
                return Err(anyhow!(
                    "expected JSON-RPC response from worker but got {} lines of \
                     non-JSON output (a library may have written to stdout during \
                     import):\n{}",
                    leaked_stdout.len(),
                    leaked_stdout.join("")
                ));
            }
        };
        if let Some(err) = resp.error {
            let detail = if let Some(tb) = &err.traceback {
                format!("rpc error {}: {}\n{tb}", err.code, err.message)
            } else {
                format!("rpc error {}: {}", err.code, err.message)
            };
            return Err(anyhow!(detail));
        }
        let val = resp.result.unwrap_or(serde_json::Value::Null);
        Ok(serde_json::from_value(val)?)
    }

    #[expect(clippy::missing_errors_doc)]
    pub async fn run_test(&mut self, test: &TestItem) -> Result<TestResult> {
        if let Some(object_path) = &test.doctest_object {
            return self.run_doctest(test, object_path).await;
        }
        let params = serde_json::to_value(RunTestParams {
            module: test.module_path.clone(),
            function: test.name.clone(),
            xfail: test.xfail.clone(),
            groups: test.groups.clone(),
            case_label: test.case_label.clone(),
        })?;
        let response: RunTestResponseWire = self.call("run_test", Some(params)).await?;
        self.latest_health = response.health.into();
        Ok(convert_result(test.clone(), response.result))
    }

    /// Send hook metadata for a module to the Python worker.
    /// Must be called before running any tests from that module.
    #[expect(clippy::missing_errors_doc)]
    pub async fn register_hooks(&mut self, params: RegisterHooksParams) -> Result<()> {
        let value = serde_json::to_value(params)?;
        self.call::<serde_json::Value>("register_hooks", Some(value))
            .await?;
        Ok(())
    }

    /// Tell the Python worker to run scope-level teardown for `per="scope"`
    /// fixtures in a module. Must be called after all tests from that module
    /// have run.
    #[expect(clippy::missing_errors_doc)]
    pub async fn finalize_hooks(&mut self, module: String) -> Result<()> {
        let value = serde_json::to_value(FinalizeHooksParams { module })?;
        self.call::<serde_json::Value>("finalize_hooks", Some(value))
            .await?;
        Ok(())
    }

    async fn run_doctest(&mut self, test: &TestItem, object_path: &str) -> Result<TestResult> {
        let params = serde_json::to_value(RunDoctestParams {
            module: test.module_path.clone(),
            object_path: object_path.to_owned(),
        })?;
        let response: RunTestResponseWire = self.call("run_doctest", Some(params)).await?;
        self.latest_health = response.health.into();
        Ok(convert_result(test.clone(), response.result))
    }

    #[expect(clippy::missing_errors_doc)]
    pub async fn ping(&mut self) -> Result<()> {
        let result: String = self.call("ping", None).await?;
        if result == "pong" {
            Ok(())
        } else {
            Err(anyhow!("unexpected ping response: {result}"))
        }
    }

    /// Snapshot the buffered worker stderr and clear the buffer.
    ///
    /// Kept `async` to preserve the pre-fix public signature; the body
    /// is purely synchronous.
    ///
    /// # Panics
    /// Panics only if the stderr-drainer task panicked while holding the
    /// internal mutex (poisoning it). That task does no fallible work.
    #[expect(
        clippy::unused_async,
        reason = "Preserves pre-fix public async signature"
    )]
    pub async fn drain_stderr(&mut self) -> String {
        let bytes: Vec<u8> = {
            let mut g = self
                .stderr_buf
                .lock()
                .expect("stderr buffer mutex poisoned");
            std::mem::take(&mut *g).into_iter().collect()
        };
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub async fn shutdown(&mut self) {
        let _ = self.child.kill().await;
    }
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        // Safety net: ensure the child process is killed when the worker is
        // dropped (e.g. on the error-respawn path in pool.rs). start_kill() is
        // the synchronous variant — safe to call on already-dead processes.
        let _ = self.child.start_kill();
    }
}

/// Spawn a tokio task that continuously reads `stderr` into `buf` until
/// EOF or a read error, capping the buffer at `STDERR_RETAIN_BYTES`.
///
/// Returns an error if no Tokio runtime is currently entered, rather than
/// panicking the way `tokio::spawn` would. This keeps the synchronous
/// `WorkerProcess::spawn` API safe to call from any context — callers in
/// non-async code receive a structured error instead of a panic.
fn spawn_stderr_drainer(
    stderr: tokio::process::ChildStderr,
    buf: Arc<Mutex<VecDeque<u8>>>,
) -> Result<()> {
    let handle = tokio::runtime::Handle::try_current()
        .map_err(|e| anyhow!("WorkerProcess::spawn requires an active tokio runtime: {e}"))?;
    handle.spawn(async move {
        let mut reader = stderr;
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => append_stderr(&buf, &chunk[..n]),
                Err(e) => {
                    trace!("stderr drainer: read error: {e}");
                    break;
                }
            }
        }
    });
    Ok(())
}

/// Append `data` to `buf`, capping it at `STDERR_RETAIN_BYTES` by
/// dropping the oldest bytes. `VecDeque::drain` removes from the front
/// in `O(excess)` time, making steady-state appends `O(data.len())`
/// rather than `O(STDERR_RETAIN_BYTES)` as a contiguous `Vec` would require.
fn append_stderr(buf: &Mutex<VecDeque<u8>>, data: &[u8]) {
    let mut g = buf.lock().expect("stderr buffer mutex poisoned");
    if data.len() >= STDERR_RETAIN_BYTES {
        // A single chunk already fills the cap; keep only its tail.
        g.clear();
        g.extend(data[data.len() - STDERR_RETAIN_BYTES..].iter().copied());
        return;
    }
    let total = g.len() + data.len();
    if total > STDERR_RETAIN_BYTES {
        let excess = total - STDERR_RETAIN_BYTES;
        g.drain(..excess);
    }
    g.extend(data.iter().copied());
}

/// Translate a resolved log level into the value placed on the spawned
/// worker's `TRYKE_LOG` env var, if any.
///
/// `Off` returns `None` so the env var stays unset and the python
/// worker's `_configure_logging_from_env` no-ops. Anything else returns
/// the lowercase level name (`debug`, `info`, ...) which the worker's
/// `logging.getLevelName` understands once uppercased.
fn worker_log_env_value(log_level: log::LevelFilter) -> Option<String> {
    if log_level == log::LevelFilter::Off {
        return None;
    }
    Some(log_level.as_str().to_ascii_lowercase())
}

fn build_pythonpath(extra: &[&Path]) -> String {
    let existing = std::env::var("PYTHONPATH").unwrap_or_default();
    let mut parts: Vec<String> = extra
        .iter()
        .map(|p| {
            let s = p
                .canonicalize()
                .unwrap_or_else(|_| p.to_path_buf())
                .to_string_lossy()
                .into_owned();
            // std::fs::canonicalize on windows produces \\?\ extended-length
            // paths that python doesn't understand
            #[cfg(windows)]
            let s = s.strip_prefix(r"\\?\").unwrap_or(&s).to_string();
            s
        })
        .collect();
    if !existing.is_empty() {
        parts.push(existing);
    }
    let sep = if cfg!(windows) { ";" } else { ":" };
    parts.join(sep)
}

fn convert_assertion(wire: AssertionWire, expected_arg_span: Option<(usize, usize)>) -> Assertion {
    let (span_offset, span_length) = compute_subject_span(&wire.expression);
    // Make absolute paths relative to cwd so diagnostics show short paths.
    let file = wire.file.map(|f| {
        std::env::current_dir()
            .ok()
            .and_then(|cwd| {
                Path::new(&f)
                    .strip_prefix(&cwd)
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned())
            })
            .unwrap_or(f)
    });
    Assertion {
        expression: wire.expression,
        file,
        line: wire.line as usize,
        span_offset,
        span_length,
        expected: wire.expected,
        received: wire.received,
        expected_arg_span,
    }
}

/// Find the byte span of the first matcher argument inside `expression`,
/// guided by AST metadata from `ExpectedAssertion`. Returns `None` for
/// no-arg matchers like `to_be_falsy()`.
fn find_arg_span(expression: &str, ea: &ExpectedAssertion) -> Option<(usize, usize)> {
    let arg = ea.args.first()?;
    if arg.is_empty() {
        return None;
    }
    // Find `matcher(` after the subject portion
    let matcher_pat = format!("{}(", ea.matcher);
    let matcher_pos = expression.find(&matcher_pat)?;
    let content_start = matcher_pos + matcher_pat.len();
    // Find the arg text right at or after the opening paren
    let remaining = expression.get(content_start..)?;
    let arg_offset_in_remaining = remaining.find(arg.as_str())?;
    let offset = content_start + arg_offset_in_remaining;
    Some((offset, arg.len()))
}

/// Find the first argument inside `expect(subject, ...)` by matching parens.
/// Stops at the first top-level comma so that optional arguments like the
/// label in `expect(val, "label")` are excluded from the span.
fn compute_subject_span(expression: &str) -> (usize, usize) {
    let Some(pos) = expression.find("expect(") else {
        return (0, expression.len().max(1));
    };
    let start = pos + 7; // len("expect(")
    let mut depth: u32 = 1;
    for (i, ch) in expression[start..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' if depth == 1 => return (start, i.max(1)),
            ')' => depth -= 1,
            ',' if depth == 1 => {
                let len = expression[start..start + i].trim_end().len();
                return (start, len.max(1));
            }
            _ => {}
        }
    }
    (start, expression.len().saturating_sub(start).max(1))
}

fn convert_result(test: TestItem, wire: RunTestResultWire) -> TestResult {
    match wire {
        RunTestResultWire::Passed {
            duration_ms,
            stdout,
            stderr,
        } => TestResult {
            test,
            outcome: TestOutcome::Passed,
            duration: Duration::from_millis(duration_ms),
            stdout,
            stderr,
        },
        RunTestResultWire::Failed {
            duration_ms,
            message,
            traceback,
            assertions,
            executed_lines,
            stdout,
            stderr,
        } => {
            let expected_by_line: std::collections::HashMap<u32, &ExpectedAssertion> = test
                .expected_assertions
                .iter()
                .map(|ea| (ea.line, ea))
                .collect();

            let assertions = assertions
                .into_iter()
                .map(|wire| {
                    let expected_arg_span = expected_by_line
                        .get(&wire.line)
                        .and_then(|ea| find_arg_span(&wire.expression, ea));
                    convert_assertion(wire, expected_arg_span)
                })
                .collect();

            TestResult {
                test,
                outcome: TestOutcome::Failed {
                    message,
                    traceback,
                    assertions,
                    executed_lines,
                },
                duration: Duration::from_millis(duration_ms),
                stdout,
                stderr,
            }
        }
        RunTestResultWire::Skipped {
            duration_ms,
            reason,
            stdout,
            stderr,
        } => TestResult {
            test,
            outcome: TestOutcome::Skipped { reason },
            duration: Duration::from_millis(duration_ms),
            stdout,
            stderr,
        },
        RunTestResultWire::XFailed {
            duration_ms,
            reason,
            stdout,
            stderr,
        } => TestResult {
            test,
            outcome: TestOutcome::XFailed { reason },
            duration: Duration::from_millis(duration_ms),
            stdout,
            stderr,
        },
        RunTestResultWire::XPassed {
            duration_ms,
            stdout,
            stderr,
        } => TestResult {
            test,
            outcome: TestOutcome::XPassed,
            duration: Duration::from_millis(duration_ms),
            stdout,
            stderr,
        },
        RunTestResultWire::Todo {
            duration_ms,
            description,
            stdout,
            stderr,
        } => TestResult {
            test,
            outcome: TestOutcome::Todo { description },
            duration: Duration::from_millis(duration_ms),
            stdout,
            stderr,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn make_test_item() -> TestItem {
        TestItem {
            name: "test_add".into(),
            module_path: "tests.test_math".into(),
            file_path: Some(PathBuf::from("tests/test_math.py")),
            line_number: Some(5),
            display_name: None,
            expected_assertions: vec![],
            ..Default::default()
        }
    }

    #[test]
    fn evaluate_recycle_returns_none_when_no_signals_tripped() {
        // A fresh worker (default health, zero age) under default
        // limits has nothing to report — the runner must not recycle.
        assert_eq!(
            evaluate_recycle(
                WorkerHealth::default(),
                Duration::ZERO,
                WorkerLimits::default(),
            ),
            None,
        );
    }

    #[test]
    fn evaluate_recycle_prioritises_memory_then_fds_then_age() {
        // All three signals tripped at once: memory wins. This
        // priority matters because memory pressure is the most
        // user-visible failure mode (process death, swap thrash); the
        // post-mortem log line should attribute that, not whichever
        // signal happened to be checked last.
        let limits = WorkerLimits {
            max_rss_bytes: Some(1000),
            max_open_fds: Some(10),
            max_age: Some(Duration::from_secs(1)),
        };
        let health = WorkerHealth {
            rss_bytes: Some(2000),
            open_fds: Some(20),
        };
        assert_eq!(
            evaluate_recycle(health, Duration::from_secs(2), limits),
            Some(RecycleReason::MemoryBytes(2000)),
        );

        // Drop the memory signal: FDs win over age.
        let health_no_mem = WorkerHealth {
            rss_bytes: None,
            open_fds: Some(20),
        };
        assert_eq!(
            evaluate_recycle(health_no_mem, Duration::from_secs(2), limits),
            Some(RecycleReason::OpenFds(20)),
        );

        // Drop FDs too: age is the last fallback.
        let health_age_only = WorkerHealth {
            rss_bytes: None,
            open_fds: None,
        };
        assert_eq!(
            evaluate_recycle(health_age_only, Duration::from_secs(2), limits),
            Some(RecycleReason::Age(Duration::from_secs(2))),
        );
    }

    #[test]
    fn evaluate_recycle_skips_signals_with_no_limit() {
        // `WorkerLimits::unlimited` opts out of every cap — even an
        // OOM-scale RSS reading must not trigger a recycle. This is
        // the contract tests rely on when exercising one signal in
        // isolation.
        let unlimited = WorkerLimits::unlimited();
        let health = WorkerHealth {
            rss_bytes: Some(u64::MAX),
            open_fds: Some(u64::MAX),
        };
        assert_eq!(
            evaluate_recycle(health, Duration::from_secs(86_400), unlimited),
            None,
        );
    }

    #[test]
    fn evaluate_recycle_skips_signals_with_no_reading() {
        // Worker on a platform without `/proc/self/fd` reports
        // `open_fds: None` — the runner must not synthesize a value
        // and must not recycle on that signal even with a tight cap.
        let limits = WorkerLimits {
            max_rss_bytes: Some(1000),
            max_open_fds: Some(10),
            max_age: None,
        };
        let health = WorkerHealth {
            rss_bytes: None,
            open_fds: None,
        };
        assert_eq!(evaluate_recycle(health, Duration::ZERO, limits), None);
    }

    #[test]
    fn recycle_reason_display_is_human_readable() {
        // The Display impl lands in debug logs ("recycling worker
        // (memory=...)") so it needs to be terse and parseable at a
        // glance — not just Debug-derived noise.
        assert_eq!(
            RecycleReason::MemoryBytes(1024).to_string(),
            "memory=1024 bytes",
        );
        assert_eq!(RecycleReason::OpenFds(42).to_string(), "open_fds=42");
        assert_eq!(
            RecycleReason::Age(Duration::from_millis(2_500)).to_string(),
            "age=2.5s",
        );
    }

    #[test]
    fn worker_log_env_value_off_returns_none() {
        // `Off` means: don't set TRYKE_LOG on the child env, preserving
        // the python worker's "no chatter unless asked" default.
        assert_eq!(worker_log_env_value(log::LevelFilter::Off), None);
    }

    #[test]
    fn worker_log_env_value_lowercases_level_name() {
        // The python worker uppercases before passing to
        // `logging.getLevelName`, so case actually doesn't matter, but
        // shipping lowercase keeps env values consistent with how rust
        // log levels render and avoids a 50/50 stylistic decision.
        assert_eq!(
            worker_log_env_value(log::LevelFilter::Info).as_deref(),
            Some("info"),
        );
        assert_eq!(
            worker_log_env_value(log::LevelFilter::Debug).as_deref(),
            Some("debug"),
        );
        assert_eq!(
            worker_log_env_value(log::LevelFilter::Warn).as_deref(),
            Some("warn"),
        );
    }

    #[test]
    fn convert_result_passed() {
        let test = make_test_item();
        let wire = RunTestResultWire::Passed {
            duration_ms: 10,
            stdout: "out".into(),
            stderr: "err".into(),
        };
        let result = convert_result(test, wire);
        assert!(matches!(result.outcome, TestOutcome::Passed));
        assert_eq!(result.duration, Duration::from_millis(10));
        assert_eq!(result.stdout, "out");
        assert_eq!(result.stderr, "err");
    }

    #[test]
    fn convert_result_failed() {
        let test = make_test_item();
        let wire = RunTestResultWire::Failed {
            duration_ms: 5,
            message: "expected 1 got 2".into(),
            traceback: None,
            assertions: vec![],
            executed_lines: vec![],
            stdout: String::new(),
            stderr: String::new(),
        };
        let result = convert_result(test, wire);
        assert!(matches!(
            result.outcome,
            TestOutcome::Failed { ref message, .. } if message == "expected 1 got 2"
        ));
    }

    #[test]
    fn convert_result_skipped() {
        let test = make_test_item();
        let wire = RunTestResultWire::Skipped {
            duration_ms: 0,
            reason: Some("not ready".into()),
            stdout: String::new(),
            stderr: String::new(),
        };
        let result = convert_result(test, wire);
        assert!(matches!(
            result.outcome,
            TestOutcome::Skipped { reason: Some(ref r) } if r == "not ready"
        ));
    }

    #[test]
    fn build_pythonpath_joins_paths() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let a = build_pythonpath(&[dir_a.path()]);
        let b = build_pythonpath(&[dir_b.path()]);
        let result = build_pythonpath(&[dir_a.path(), dir_b.path()]);
        let sep = if cfg!(windows) { ";" } else { ":" };
        assert_eq!(result, format!("{a}{sep}{b}"));
    }

    #[test]
    fn convert_assertion_maps_wire_fields() {
        let wire = AssertionWire {
            expression: "expect(x).to_equal(2)".into(),
            expected: "2".into(),
            received: "3".into(),
            line: 10,
            file: Some("tests/test_math.py".into()),
        };
        let a = convert_assertion(wire, Some((19, 1)));
        assert_eq!(a.expression, "expect(x).to_equal(2)");
        assert_eq!(a.expected, "2");
        assert_eq!(a.received, "3");
        assert_eq!(a.line, 10);
        assert_eq!(a.file.as_deref(), Some("tests/test_math.py"));
        // "expect(" is 7 chars, subject "x" starts at 7, length 1
        assert_eq!(a.span_offset, 7);
        assert_eq!(a.span_length, 1);
        assert_eq!(a.expected_arg_span, Some((19, 1)));
    }

    #[test]
    fn compute_subject_span_simple() {
        let (off, len) = compute_subject_span("expect(x).to_equal(2)");
        assert_eq!(off, 7);
        assert_eq!(len, 1);
    }

    #[test]
    fn compute_subject_span_nested_parens() {
        let (off, len) = compute_subject_span("expect(foo(bar(1))).to_be_truthy()");
        assert_eq!(off, 7);
        assert_eq!(len, 11); // "foo(bar(1))"
    }

    #[test]
    fn compute_subject_span_no_expect() {
        let (off, len) = compute_subject_span("assert x == 1");
        assert_eq!(off, 0);
        assert_eq!(len, 13);
    }

    #[test]
    fn compute_subject_span_stops_at_comma() {
        let (off, len) = compute_subject_span("expect(val, \"label\")");
        assert_eq!(off, 7);
        assert_eq!(len, 3); // "val"
    }

    #[test]
    fn compute_subject_span_nested_parens_with_label() {
        let (off, len) = compute_subject_span("expect(foo(a, b), \"label\")");
        assert_eq!(off, 7);
        assert_eq!(len, 9); // "foo(a, b)"
    }

    #[test]
    fn compute_subject_span_whitespace_before_comma() {
        let (off, len) = compute_subject_span("expect(val  , \"label\")");
        assert_eq!(off, 7);
        assert_eq!(len, 3); // "val" (trailing whitespace trimmed)
    }

    fn make_expected(matcher: &str, args: &[&str], line: u32) -> ExpectedAssertion {
        ExpectedAssertion {
            subject: "x".into(),
            matcher: matcher.into(),
            negated: false,
            args: args.iter().map(|s| (*s).to_string()).collect(),
            line,
            label: None,
        }
    }

    #[test]
    fn find_arg_span_matcher_with_arg() {
        let ea = make_expected("to_equal", &["2"], 10);
        let span = find_arg_span("expect(x).to_equal(2)", &ea);
        assert_eq!(span, Some((19, 1)));
    }

    #[test]
    fn find_arg_span_no_arg_matcher() {
        let ea = make_expected("to_be_falsy", &[], 10);
        let span = find_arg_span("expect(val).to_be_falsy()", &ea);
        assert_eq!(span, None);
    }

    #[test]
    fn find_arg_span_complex_expression() {
        let ea = ExpectedAssertion {
            subject: "foo(bar(1))".into(),
            matcher: "to_equal".into(),
            negated: false,
            args: vec!["42".into()],
            line: 10,
            label: None,
        };
        let span = find_arg_span("expect(foo(bar(1))).to_equal(42)", &ea);
        assert_eq!(span, Some((29, 2)));
    }

    #[test]
    fn find_arg_span_string_arg() {
        let ea = make_expected("to_equal", &["\"hello\""], 10);
        let span = find_arg_span("expect(x).to_equal(\"hello\")", &ea);
        assert_eq!(span, Some((19, 7)));
    }

    #[test]
    fn find_arg_span_matcher_not_found() {
        let ea = make_expected("to_contain", &["5"], 10);
        let span = find_arg_span("expect(x).to_equal(2)", &ea);
        assert_eq!(span, None);
    }

    #[tokio::test]
    async fn drop_kills_child_process() {
        let mut child = tokio::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("failed to spawn sleep");
        let pid = child.id().expect("missing pid");

        // Wrap in a WorkerProcess-like drop: start_kill then drop
        let _ = child.start_kill();
        drop(child);

        // Give the OS a moment to reap the process
        tokio::time::sleep(Duration::from_millis(50)).await;

        // On Unix, sending signal 0 checks if the process exists
        let status = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status();
        assert!(
            status.is_ok_and(|s| !s.success()),
            "child process {pid} should be dead after start_kill + drop"
        );
    }

    #[test]
    fn append_stderr_caps_at_retain_bytes() {
        let buf = Mutex::new(VecDeque::<u8>::new());
        // Fill with a recognisable head sequence, then push past the cap.
        let head = vec![b'A'; STDERR_RETAIN_BYTES];
        append_stderr(&buf, &head);
        let tail = vec![b'B'; 1024];
        append_stderr(&buf, &tail);

        let mut g = buf.lock().unwrap();
        assert_eq!(g.len(), STDERR_RETAIN_BYTES);
        // Oldest A's are dropped; tail B's retained.
        let bytes = g.make_contiguous();
        assert_eq!(&bytes[bytes.len() - tail.len()..], &tail[..]);
        assert_eq!(bytes[0], b'A');
    }

    #[test]
    fn append_stderr_handles_single_oversized_write() {
        let buf = Mutex::new(VecDeque::<u8>::new());
        let big = vec![b'X'; STDERR_RETAIN_BYTES + 4096];
        append_stderr(&buf, &big);
        let mut g = buf.lock().unwrap();
        assert_eq!(g.len(), STDERR_RETAIN_BYTES);
        assert!(g.make_contiguous().iter().all(|b| *b == b'X'));
    }

    /// Reproducer for the "tryke hangs at `finalize_hooks`" bug: when the
    /// worker writes more than the kernel pipe buffer (~64 KiB on Linux,
    /// ~4 KiB on Windows) to stderr without anyone draining it, the
    /// worker's next stderr write blocks and it stops responding to
    /// RPCs. The drainer task spawned in `WorkerProcess::spawn` must
    /// keep the pipe empty so RPC flow is unaffected.
    ///
    /// Test contract is binary: either the drainer prevents the deadlock
    /// and `done` arrives on stdout, or it doesn't. The cap-retention
    /// property is covered by the `append_stderr_*` unit tests above and
    /// is intentionally NOT asserted here — re-asserting it via a second
    /// post-exit polling loop introduced wall-clock flakiness on slow
    /// Windows runners without adding any signal a unit test wouldn't
    /// catch.
    ///
    /// Uses `python3` (already a project dependency) for portability and
    /// routes through the same `spawn_stderr_drainer` helper as
    /// `WorkerProcess::spawn`, so a regression in the production drainer
    /// path would deadlock this test too.
    #[tokio::test]
    async fn worker_continues_after_large_stderr_write() {
        // 256 KiB is comfortably above both Linux's 64 KiB pipe buffer
        // and Windows's 4 KiB default — enough to deadlock without a
        // drainer. Larger values (we used to write 1 MiB) just make the
        // test slower without proving anything additional.
        const STDERR_BYTES: usize = 256 * 1024;
        let mut child = tokio::process::Command::new("python3")
            .args([
                "-c",
                &format!(
                    "import sys; sys.stderr.write('X' * {STDERR_BYTES}); \
                     sys.stderr.flush(); print('done', flush=True)"
                ),
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn python3");

        let stderr = child.stderr.take().expect("no stderr");
        let stderr_buf: Arc<Mutex<VecDeque<u8>>> = Arc::new(Mutex::new(VecDeque::new()));
        spawn_stderr_drainer(stderr, Arc::clone(&stderr_buf))
            .expect("drainer requires tokio runtime");

        let mut stdout = BufReader::new(child.stdout.take().expect("no stdout"));
        let mut line = String::new();
        // 30s catches a real deadlock (which would never finish) while
        // tolerating Windows python startup + scheduler jitter. Past 5s
        // budgets occasionally tripped on Windows × 3.14 even though no
        // deadlock occurred.
        tokio::time::timeout(Duration::from_secs(30), stdout.read_line(&mut line))
            .await
            .expect("stdout read timed out — drainer deadlocked")
            .expect("stdout read errored");
        assert_eq!(line.trim(), "done");

        let _ = child.wait().await;
    }
}
