# Configuration

Tryke is configured via `pyproject.toml` under the `[tool.tryke]` table.

## `pyproject.toml`

```toml
[tool.tryke]
exclude = ["benchmarks/", "scripts/generate.py"]
```

### `exclude`

A list of file paths or directory patterns to exclude from test discovery:

```toml
[tool.tryke]
exclude = [
    "benchmarks/",
    "scripts/",
    "tests/generated/",
]
```

Excluded paths are skipped during both test collection and import graph construction.

### `src`

A list of source roots used to resolve absolute imports against the project's files. Defaults to `["."]` — the project root — which is correct for projects whose packages live next to `pyproject.toml`.

For layouts that put the package tree under a subdirectory (for example a maturin project with `python-source = "python"`, where the package lives at `python/mypkg/`), list that subdirectory so absolute imports in test files resolve to the right source file:

```toml
[tool.tryke]
src = [".", "python"]
```

With `src = [".", "python"]`, `from mypkg.mod import X` in a test file is tried as `./mypkg/mod.py` first and then `python/mypkg/mod.py` — matching how `sys.path` layers multiple package roots.

Roots earlier in the list take precedence. Roots that don't resolve to a file on disk are skipped silently, so listing `"."` alongside a subdirectory is safe.

This only affects absolute imports (`from foo.bar import x`). Relative imports (`from .sibling import x`) always resolve from the importing file's directory and are unaffected.

### `python`

Path to the Python interpreter used to spawn worker processes. Tryke does not enforce `requires-python` — that is the package manager's job (uv, pip, poetry, hatch). Whatever interpreter you point at is the one that runs your tests.

```toml
[tool.tryke]
python = ".venv/bin/python3"
```

Defaults to `python` on Windows and `python3` on Unix from `PATH`.

**Path resolution.** A value with a path separator (e.g., `.venv/bin/python3`) is treated as a filesystem path; bare names (e.g., `python3`, `pypy`) are looked up via `PATH` exactly like `execvp` / `CreateProcess`. Relative paths are anchored to the directory containing `pyproject.toml`, not the cwd, so `python = ".venv/bin/python3"` keeps working when tryke is invoked from a sibling directory or a script. Absolute paths and Windows drive-relative values (e.g., `C:foo\python.exe`) are passed through unchanged.

## CLI overrides

### `--exclude` / `-e`

Override the `pyproject.toml` exclude list from the command line:

```bash
tryke test --exclude benchmarks/ --exclude scripts/
```

Note: `--exclude` **replaces** the config file setting, it does not extend it.

### `--include` / `-i`

Include files or directories that would otherwise be excluded by `pyproject.toml`:

```bash
tryke test --include tests/legacy/
```

This is useful for one-off runs against normally excluded paths without editing the config file.

### `--root`

Override the project root (where Tryke looks for `pyproject.toml` and test files):

```bash
tryke test --root /path/to/project
```

## Logging

Tryke has a single user-facing verbosity knob with a precedence chain spanning CLI flags, environment variables, and cross-language propagation to the python workers it spawns.

### CLI flags

`-v`, `-vv`, `-vvv` raise the level (info → debug → trace). `-q`, `-qq` lower it (error → silent). The default is `warn`.

### Environment variables

- **`TRYKE_LOG`** — the umbrella knob. Accepts a bare level name (`off`, `error`, `warn`, `info`, `debug`, `trace`) and propagates to **both** the rust process and every python worker it spawns. This is what you should set when you want one knob.
- **`RUST_LOG`** — power-user override for the rust side only. Honored natively by `env_logger`, so the standard per-module filter syntax (`tryke=debug,hyper=warn`) works. Does **not** propagate to python workers — its module-filter grammar doesn't map onto a python log level.

### Precedence

**Rust log filter** (consumed by `env_logger`):

1. `RUST_LOG` if set (wins natively).
2. `TRYKE_LOG` if set.
3. The CLI flag (`-v` / `-q`).
4. Default `warn`.

**Python worker log** (spawned by tryke, configured by `TRYKE_LOG` on the worker env):

1. `TRYKE_LOG` if set.
2. The CLI flag, **only** when explicitly more verbose than `warn` (i.e., the user passed at least one `-v`). Default `warn` does not light up workers — preserves the long-standing "no chatter unless asked" behavior.
3. Otherwise off.

### Examples

```bash
# Default: rust at warn, workers silent.
tryke test

# `-v` lights up both layers at info.
tryke -v test

# Per-module rust filtering, workers stay silent.
RUST_LOG=tryke=debug,tryke_runner=trace tryke test

# Single knob: both layers at debug, regardless of CLI flag.
TRYKE_LOG=debug tryke test

# RUST_LOG wins for rust filtering; TRYKE_LOG still drives python.
TRYKE_LOG=info RUST_LOG=tryke=warn tryke test
```

The legacy `TRYKE_WORKER_LOG` env var is still accepted (one cycle) as a deprecated alias for `TRYKE_LOG`. A single deprecation line is logged when only the old name is set.

## Example

A typical configuration for a project with benchmarks and generated code:

```toml
[tool.tryke]
exclude = [
    "benchmarks/",
    "scripts/codegen/",
    "tests/fixtures/generated/",
]
```

```bash
# Normal run — respects pyproject.toml excludes
tryke test

# One-off: include the benchmarks
tryke test --include benchmarks/

# One-off: different exclude set
tryke test --exclude tests/slow/
```
