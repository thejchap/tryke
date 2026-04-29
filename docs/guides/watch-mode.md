# Watch mode

Watch mode monitors your project for file changes and reruns only the affected tests.

## Basic usage

```bash
tryke test --watch
```

Tryke watches all `.py` files in the project, respecting `.gitignore`. When a file changes, it:

1. Identifies which modules were modified
2. Walks the import graph to find all tests that depend on the changed modules
3. Restarts the worker subprocesses so the next run loads the updated code in a fresh Python interpreter
4. Reruns only the affected tests

This gives you fast feedback without rerunning the entire suite. Restarting the workers (rather than calling `importlib.reload` in-process) avoids the classic reload pitfalls — stale class objects, captured closures, and decorator-bound state from the old definitions are all dropped because the interpreter itself is gone.

## How affected tests are determined

Tryke builds a static import graph at startup (see [test discovery](../concepts/discovery.md)). When a file changes, it traces the graph forward to find every test file that transitively imports the changed module, then reruns the tests in those files.

Files with dynamic imports (`importlib.import_module()`, `__import__()`) are always included in every rerun. See [discovery](../concepts/discovery.md#what-happens-when-dynamic-imports-are-detected) for details.

## Filtering

All the standard [filtering](filtering.md) flags work in watch mode:

```bash
# Only watch tests matching a name pattern
tryke test --watch -k "math"

# Only watch tests with specific tags
tryke test --watch -m "fast"

# Combine filters
tryke test --watch -k "parse" -m "not slow"
```

## Options

### Reporter

Choose an output format:

```bash
tryke test --watch --reporter dot
```

See [reporters](reporters.md) for all formats.

### Fail fast

Stop a run on the first failure:

```bash
tryke test --watch -x
```

Or after N failures:

```bash
tryke test --watch --maxfail 3
```

### Workers

Override the number of parallel workers (defaults to CPU count):

```bash
tryke test --watch -j 4
```

See [concurrency](../concepts/concurrency.md) for details on the worker pool.

### Run all tests on every change

By default, watch mode reruns only the tests affected by the changed files
(via the import graph). Pass `--all` (short: `-a`) to rerun the full
discovered test set on every change instead:

```bash
tryke test --watch --all
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

## Debouncing and change dedup

File system events are debounced with a 50ms quiet window — just long enough to coalesce the burst of inotify events the kernel emits for a single write syscall.

On top of the debouncer, watch mode tracks each file's `(mtime, size)` signature and skips events that don't actually move it. Editor tail activity (atomic-rename metadata writes, swap-file cleanup, format-on-save that produces identical output, LSP re-saves) often produces a second batch of events outside the debounce window; without the signature check, that second batch would trigger a redundant restart for a single user save. With it, only batches that represent a real content change reach the worker pool — so we can keep the debounce tight without paying for it in spurious restarts.
