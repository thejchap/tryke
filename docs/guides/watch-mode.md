# Watch mode

Watch mode monitors your project for file changes and reruns only the affected tests.

## Basic usage

```bash
tryke watch
```

Tryke watches all `.py` files in the project, respecting `.gitignore`. When a file changes, it:

1. Identifies which modules were modified
2. Walks the import graph to find all tests that depend on the changed modules
3. Reruns only those tests

This gives you fast feedback without rerunning the entire suite.

## How affected tests are determined

Tryke builds a static import graph at startup (see [test discovery](../concepts/discovery.md)). When a file changes, it traces the graph forward to find every test file that transitively imports the changed module, then reruns the tests in those files.

Files with dynamic imports (`importlib.import_module()`, `__import__()`) are always included in every rerun. See [discovery](../concepts/discovery.md#what-happens-when-dynamic-imports-are-detected) for details.

## Filtering

All the standard [filtering](filtering.md) flags work in watch mode:

```bash
# Only watch tests matching a name pattern
tryke watch -k "math"

# Only watch tests with specific tags
tryke watch -m "fast"

# Combine filters
tryke watch -k "parse" -m "not slow"
```

## Options

### Reporter

Choose an output format:

```bash
tryke watch --reporter dot
```

See [reporters](reporters.md) for all formats.

### Fail fast

Stop a run on the first failure:

```bash
tryke watch -x
```

Or after N failures:

```bash
tryke watch --maxfail 3
```

### Workers

Override the number of parallel workers (defaults to CPU count):

```bash
tryke watch -j 4
```

See [concurrency](../concepts/concurrency.md) for details on the worker pool.

### Run all tests on every change

By default, watch mode reruns only the tests affected by the changed files
(via the import graph). Pass `--all` (short: `-a`) to rerun the full
discovered test set on every change instead:

```bash
tryke watch --all
```

This is useful when:

- The import graph misses a dependency the test relies on (dynamic imports,
  plugin registries, fixtures wired up at runtime, string-referenced modules).
- An external resource (schema, fixture file, environment variable) changed
  in a way the import graph cannot see.
- You are debugging test ordering or flake issues and want a full run on
  every save.

Worker subprocesses are still restarted on every change so Python picks up
the new code from a fresh interpreter; only the test selection is broadened.

## Debouncing

File system events are debounced with a 200ms window. Rapid successive saves are coalesced into a single rerun.
