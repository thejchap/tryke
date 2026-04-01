# Concurrency

tryke runs tests in parallel using a pool of Python worker processes managed by the Rust runtime.

## Worker pool

Each worker is a separate Python process that communicates with the Rust runtime over stdin/stdout using JSON-RPC. Workers are spawned once and reused across tests — there's no per-test process overhead.

### Default worker count

The pool size depends on the context:

| Context | Default |
|---------|---------|
| `tryke test` | `min(test_count, cpu_count)` |
| `tryke watch` | `cpu_count` |
| `tryke server` | `cpu_count` |

If CPU detection fails, the fallback is 4 workers. Override with `-j` / `--workers`:

```bash
tryke test -j 4
```

The `tryke test` default avoids creating more workers than there are tests to run.

## Pre-warming

Workers are pre-warmed at startup. Before any tests execute, tryke sends a ping to every worker, causing each one to spawn its Python subprocess in parallel. This means Python startup latency is absorbed before the first test begins, not during it.

## Distribution modes

The `--dist` flag controls how tests are partitioned into work units before scheduling:

| Mode | Granularity | Use case |
|------|-------------|----------|
| `test` (default) | Each test is its own work unit | Maximum parallelism; best for CPU-bound tests with no shared state |
| `file` | All tests from a file form one work unit | Tests share module-level state or rely on ordering within a file |
| `group` | Tests within a `describe()` block form one work unit | Isolation within describe groups while still parallelizing across them |

```bash
tryke test --dist file
tryke test --dist group
```

Work units are sorted largest-first before scheduling so the longest pole starts early.

## Scheduling

Work units are distributed to workers using lock-free round-robin scheduling. Each unit is assigned to the next worker in sequence, wrapping around after the last. This keeps the distribution even without requiring synchronization between workers.

## Output buffering

Test results are buffered per file and reported in discovery order. Even though tests from different files may complete out of order, the output groups results by file and preserves the order tests were defined in.

This means the output is deterministic regardless of scheduling — the same tests always produce the same report order.

### How it works

1. As results stream in from workers, they accumulate in a per-file buffer
2. When all tests from a file are complete, the buffer is flushed in discovery order
3. If `--maxfail` or `-x` stops execution early, remaining buffered results are flushed before exiting

## Hooks and scheduling

Lifecycle hooks interact with the distribution mode. The key constraint:
`@before_all`, `@after_all`, and `@wrap_all` cache their return values
in the worker's Python process. All tests that share a cached value must
run on the same worker.

When tryke detects `_all` hooks during discovery, it automatically
upgrades the distribution mode:

| Hooks present | Requested mode | Effective mode | Why |
|---------------|----------------|----------------|-----|
| None or `_each` only | `test` | `test` | No shared state — full parallelism |
| `@before_all` at file scope | `test` | `file` | Cached value must stay in one process |
| `@before_all` in describe | `test` | `group` | Only that group's tests need the value |
| `@before_all` at file scope | `group` | `file` | File-scope `_all` hooks require the whole file on one worker |
| `_all` hooks only inside `describe` | `group` | `group` | Groups are already kept together |
| Any | `file` | unchanged | Already grouped at file granularity — no upgrade needed |

`@before_each`, `@after_each`, and `@wrap_each` hooks do not constrain
scheduling. Their values are created fresh per test and discarded
afterward, so tests can run on any worker.

### Practical impact

A file with only `@before_each` hooks keeps full `--dist test` parallelism — every test can run on a different worker. Adding a single `@before_all` at module scope forces all tests in that file onto one worker. If this is a bottleneck, consider scoping the `@before_all` inside a `describe` block so only that group is constrained.

## Isolation

Each worker process has its own Python interpreter and module cache. Tests in different workers cannot interfere with each other through global state, module-level side effects, or shared mutable imports.

If your tests modify global state that other tests depend on, consider whether those dependencies should be made explicit or restructured.
