//! JSON-RPC wire format spoken between the Rust runner and the long-lived
//! Python worker process at `python/tryke/worker.py`.
//!
//! Each struct in this module is one half of a message that flows over the
//! worker's stdin/stdout. A typical test cycle looks like:
//!
//! 1. `register_hooks`   — Rust side sends a [`RegisterHooksParams`] with a
//!    list of [`HookWire`] entries for one module. The worker stores the raw
//!    metadata on `Worker._hook_metadata` without importing the module yet;
//!    the import is deferred until the first test in that module actually
//!    runs, so collection stays cheap.
//! 2. `run_test`         — Rust sends one [`RunTestParams`] per test. On
//!    first touch of a module the worker imports it, then lazily builds a
//!    `HookExecutor` by looking up each hook name as an attribute of the
//!    imported module and reading its `per=test`/`per=scope` kind. Fixtures
//!    are resolved and injected before the test function runs.
//! 3. `finalize_hooks`   — after the last test in a module, Rust sends
//!    [`FinalizeHooksParams`] so `per="scope"` teardown runs.
//! 4. `reload` (watch/server mode only) — [`ReloadParams`] tells the worker
//!    to drop any cached import of the listed dotted module names and
//!    invalidate their cached executors. Next test triggers a fresh import.
//!
//! Because hooks are discovered statically by Ruff (not by importing), the
//! runner knows every `@fixture` name and every `Depends(...)` reference
//! before any Python code runs, and ships that as the wire payload. The
//! worker never needs to re-walk the AST itself.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct RpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct RpcResponse {
    pub id: u64,
    pub result: Option<serde_json::Value>,
    pub error: Option<RpcErrorDetail>,
}

#[derive(Debug, Deserialize)]
pub struct RpcErrorDetail {
    pub code: i32,
    pub message: String,
    #[serde(default)]
    pub traceback: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RunTestParams {
    pub module: String,
    pub function: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xfail: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    /// For `@test.cases(...)` items, the label of the case to dispatch.
    /// The worker looks up `fn.__tryke_cases__[case_label]` and passes the
    /// stored kwargs when invoking the test function.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub case_label: Option<String>,
}

/// Wire format for a single fixture sent to the Python worker.
///
/// Populated on the Rust side from a statically-discovered
/// [`tryke_types::HookItem`]. The Python worker ([`Worker._register_hooks`]
/// in `python/tryke/worker.py`) stores these verbatim and later resolves
/// each `name` to a real function by `getattr`-ing the imported module,
/// so the Python side never re-parses source.
///
/// Field notes:
///
/// - `name` — bare function name as written in source; the worker looks
///   this up as an attribute on the imported module.
/// - `per` — serialized [`tryke_types::FixturePer`] (`"test"` or `"scope"`);
///   controls fixture lifetime and, for `"scope"`, scheduling affinity.
/// - `groups` — `describe(...)` path the fixture was defined under, used
///   to scope fixture visibility to tests inside the same group chain.
/// - `depends_on` — function names extracted from `Depends(name)` in the
///   hook's parameter defaults. The worker uses this to build its DI
///   graph; unknown names become a runtime error when the fixture is
///   first requested.
/// - `line_number` — source line for diagnostics.
///
/// [`Worker._register_hooks`]: # "see python/tryke/worker.py"
#[derive(Debug, Clone, Serialize)]
pub struct HookWire {
    pub name: String,
    pub per: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    pub line_number: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegisterHooksParams {
    pub module: String,
    pub hooks: Vec<HookWire>,
}

#[derive(Debug, Serialize)]
pub struct FinalizeHooksParams {
    pub module: String,
}

#[derive(Debug, Serialize)]
pub struct RunDoctestParams {
    pub module: String,
    pub object_path: String,
}

#[derive(Debug, Serialize)]
pub struct ReloadParams {
    pub modules: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RunTestResultWire {
    Passed {
        duration_ms: u64,
        stdout: String,
        stderr: String,
    },
    Failed {
        duration_ms: u64,
        message: String,
        #[serde(default)]
        traceback: Option<String>,
        #[serde(default)]
        assertions: Vec<AssertionWire>,
        #[serde(default)]
        executed_lines: Vec<u32>,
        stdout: String,
        stderr: String,
    },
    Skipped {
        duration_ms: u64,
        #[serde(default)]
        reason: Option<String>,
        stdout: String,
        stderr: String,
    },
    #[serde(rename = "xfailed")]
    XFailed {
        duration_ms: u64,
        #[serde(default)]
        reason: Option<String>,
        stdout: String,
        stderr: String,
    },
    #[serde(rename = "xpassed")]
    XPassed {
        duration_ms: u64,
        stdout: String,
        stderr: String,
    },
    Todo {
        duration_ms: u64,
        #[serde(default)]
        description: Option<String>,
        stdout: String,
        stderr: String,
    },
}

#[derive(Debug, Deserialize)]
pub struct AssertionWire {
    pub expression: String,
    pub expected: String,
    pub received: String,
    pub line: u32,
    #[serde(default)]
    pub file: Option<String>,
}
