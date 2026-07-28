# Configuration

Tryke is configured via `pyproject.toml` under the `[tool.tryke]` table.

## `pyproject.toml`

```toml
[tool.tryke]
exclude = ["scripts/generated.py", "tests/fixtures/generated/"]
```

### `exclude`

A list of file paths or directory patterns to exclude from test discovery:

```toml
[tool.tryke]
exclude = [
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

Path to the Python interpreter or environment used to spawn worker processes. Tryke does not enforce `requires-python` — that is the package manager's job (uv, pip, poetry, hatch). Whatever interpreter you point at is the one that runs your tests.

```toml
[tool.tryke]
python = ".venv/bin/python3"
```

An environment directory is also accepted:

```toml
[tool.tryke]
python = ".venv"
```

When `python` is unset, Tryke discovers an environment in this order:

1. `VIRTUAL_ENV`
2. An active non-base Conda environment
3. `.venv` in the project root
4. An active base Conda environment
5. `python` on Windows or `python3` on Unix from `PATH`

**Path resolution.** A value with a path separator (e.g., `.venv/bin/python3`) is treated as a filesystem path; bare names (e.g., `python3`, `pypy`) are looked up via `PATH` exactly like `execvp` / `CreateProcess`. Relative paths from `pyproject.toml` are anchored to the directory containing that file, not the cwd. Relative paths passed via `--python` are anchored to the project root. Paths are made absolute without resolving symlinks, preserving virtual-environment interpreter identity. Absolute paths and Windows drive-relative values (e.g., `C:foo\\python.exe`) are passed through unchanged.

### `cache_dir`

Directory for tryke's persistent discovery cache. By default, tryke stores discovery results under `<project-root>/.tryke/cache`; set `cache_dir` when that location is not suitable (for example, a read-only project checkout or a shared CI cache directory).

```toml
[tool.tryke]
cache_dir = ".cache/tryke"
```

Relative paths are anchored to the directory containing `pyproject.toml`, not the cwd. The command-line `--cache-dir` flag takes precedence for one-off runs.

## CLI overrides

### `--exclude` / `-e`

Override the `pyproject.toml` exclude list from the command line:

```bash
tryke test --exclude scripts/ --exclude tests/fixtures/generated/
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

### `--cache-dir`

Override the discovery cache directory from the command line:

```bash
tryke --cache-dir /tmp/tryke-cache test
tryke server --cache-dir .cache/tryke
```

`--cache-dir` is a global flag and overrides `[tool.tryke] cache_dir`. Relative CLI paths are resolved by the shell/process cwd.

## Logging

Tryke resolves one user-facing log level and applies it to Rust logging, reporter diagnostics, and every Python worker it spawns.

### CLI flags

`-v`, `-vv`, `-vvv` raise the logging and diagnostic level (info → debug → trace). `-q`, `-qq` lower it (error → silent). The default is `warn`. The text reporter shows per-expectation lines by default; `-v` is not required for that output.

### Environment variables

- **`TRYKE_LOG`** — the umbrella knob. Accepts a bare level name (`off`, `error`, `warn`, `info`, `debug`, `trace`) and overrides the CLI level everywhere. Values are case-insensitive and may have surrounding whitespace. An invalid value stops the command with an error instead of being silently ignored.
- **`RUST_LOG`** — power-user override for the Rust side only. Honored natively by `env_logger`, so the standard per-module filter syntax (`tryke=debug,hyper=warn`) works. Does **not** propagate to Python workers — its module-filter grammar doesn't map onto a Python log level.

### Precedence

**Resolved Tryke level**:

1. `TRYKE_LOG` if set.
2. The CLI flag (`-v` / `-q`).
3. Default `warn`.

This level drives reporter diagnostics and is passed to Python workers as `TRYKE_LOG`. Python's standard library has no trace level, so workers map `trace` to `debug`.

The resolved level is also the default Rust filter. If `RUST_LOG` is set, it overrides that filter for Rust logging only; reporter and worker verbosity continue to use the resolved Tryke level.

### Examples

```bash
# Default: Rust and workers at warn.
tryke test

# `-v` raises Rust, reporter, and worker verbosity to info.
tryke -v test

# Per-module Rust filtering; reporter and workers remain at warn.
RUST_LOG=tryke=debug,tryke_runner=trace tryke test

# Single knob: both layers at debug, regardless of CLI flag.
TRYKE_LOG=debug tryke test

# RUST_LOG wins for Rust filtering; TRYKE_LOG still drives Python.
TRYKE_LOG=info RUST_LOG=tryke=warn tryke test
```

## Example

A typical configuration for a project with generated code and large fixtures:

```toml
[tool.tryke]
exclude = [
    "scripts/codegen/",
    "tests/fixtures/generated/",
]
```

```bash
# Normal run — respects pyproject.toml excludes
tryke test

# One-off: include normally excluded generated fixtures
tryke test --include tests/fixtures/generated/

# One-off: different exclude set
tryke test --exclude tests/slow/
```
