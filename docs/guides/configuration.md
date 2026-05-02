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
