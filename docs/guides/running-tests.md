# Running tests

## Basic usage

Run all tests in your project:

```bash
tryke test
```

Or without installing:

```bash
uvx tryke test
```

## Specifying paths

Pass one or more file or directory paths to restrict which tests are collected:

```bash
tryke test tests/test_math.py
tryke test tests/unit/ tests/integration/
```

### `file:line` syntax

Jump to a specific test by pointing at the line where it's defined:

```bash
tryke test tests/test_math.py:12
```

tryke runs the test whose definition spans that line. This is especially useful from editor integrations that can resolve the cursor position.

## Collecting without running

Use `--collect-only` to list discovered tests without executing them:

```bash
tryke test --collect-only
```

This is useful for verifying [filtering](filtering.md) expressions or checking that tryke sees your tests.

## Stopping on failure

Stop after the first failure with `-x` / `--fail-fast`:

```bash
tryke test -x
```

Stop after N failures with `--maxfail`:

```bash
tryke test --maxfail 3
```

## Parallel execution

tryke runs tests in parallel by default. The worker count defaults to `min(test_count, cpu_count)`. Override with `-j` / `--workers`:

```bash
tryke test -j 4
```

### Distribution mode

By default, each test is its own work unit and can run on any worker (`--dist test`). Use `--dist` to control how tests are partitioned across workers:

```bash
tryke test --dist file    # All tests from a file go to one worker
tryke test --dist group   # Tests within a describe() group go to one worker
tryke test --dist test    # Each test is independent (default)
```

`file` mode is useful when tests share module-level state. `group` mode balances between parallelism and isolation within describe blocks.

See [concurrency](../concepts/concurrency.md) for details on the worker pool model.

## Project root

By default tryke uses the current directory as the project root. Override with `--root`:

```bash
tryke test --root /path/to/project
```

The root determines where tryke looks for `pyproject.toml`, test files, and the import graph.

## Filtering

See the [filtering guide](filtering.md) for `-k` expressions, `-m` tag filters, and how to combine them.

## Reporter format

Choose an output format with `--reporter`:

```bash
tryke test --reporter dot
tryke test --reporter json
```

See the [reporters guide](reporters.md) for all available formats.

## Connecting to a server

If you have a [tryke server](../concepts/client-server.md) running, connect to it for faster runs:

```bash
tryke test --port
tryke test --port 2337
```

The server keeps Python workers warm and caches test discovery, so subsequent runs skip startup overhead.
