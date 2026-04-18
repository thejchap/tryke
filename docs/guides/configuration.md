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
    "tests/legacy/",
]
```

Excluded paths are skipped during both test collection and import graph construction.

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
