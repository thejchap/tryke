# Concurrency

Tryke runs tests in parallel using a pool of Python worker processes managed by the Rust runtime.

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

Workers are pre-warmed at startup. Before any tests execute, Tryke sends a ping to every worker, causing each one to spawn its Python subprocess in parallel. This means Python startup latency is absorbed before the first test begins, not during it.

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

## Fixtures and scheduling

Fixtures interact with the distribution mode. The key constraint:
`@fixture(per="scope")` caches its return value in the worker's Python
process. All tests that share a cached value must run on the same worker.

When Tryke detects `per="scope"` fixtures during discovery, it
automatically upgrades the distribution mode:

| Fixtures present | Requested mode | Effective mode | Why |
|------------------|----------------|----------------|-----|
| None or `per="test"` only | `test` | `test` | No shared state — full parallelism |
| `per="scope"` at file scope | `test` | `file` | Cached value must stay in one process |
| `per="scope"` in describe | `test` | `group` | Only that group's tests need the value |
| `per="scope"` at file scope | `group` | `file` | File-scope `per="scope"` requires the whole file on one worker |
| `per="scope"` only inside `describe` | `group` | `group` | Groups are already kept together |
| Any | `file` | unchanged | Already grouped at file granularity — no upgrade needed |

`per="test"` fixtures do not constrain scheduling. Their values are
created fresh per test and discarded afterward, so tests can run on any
worker.

### Practical impact

A file with only `per="test"` fixtures keeps full `--dist test` parallelism — every test can run on a different worker. Adding a single `per="scope"` fixture at module scope forces all tests in that file onto one worker. If this is a bottleneck, consider scoping the fixture inside a `describe` block so only that group is constrained.

## Isolation

Each worker process has its own Python interpreter and module cache. Tests in different workers cannot interfere with each other through global state, module-level side effects, or shared mutable imports.

If your tests modify global state that other tests depend on, consider whether those dependencies should be made explicit or restructured.

### Same-worker sharing of `per="scope"` values

Cross-worker isolation does not extend to tests *within the same worker*. When `@fixture(per="scope")` caches a value, every test in that scope running on that worker receives **the same object by reference**. Mutating it is observable by subsequent tests in the scope, exactly as if it were a module-level global.

```python
@fixture(per="scope")
def config() -> dict[str, str]:
    return {"env": "test"}

@test
def first(cfg: dict[str, str] = Depends(config)) -> None:
    cfg["env"] = "mutated"  # Leaks to later tests in this scope.

@test
def second(cfg: dict[str, str] = Depends(config)) -> None:
    # Sees {"env": "mutated"} if first() ran on the same worker.
    ...
```

This is intentional — it's the whole point of scope-level fixtures (set up an expensive resource once, share it). But it means `per="scope"` values should either be treated as read-only or represent resources whose mutation is part of their contract (database connections, temp directories).

Recommended patterns:

- Return immutable values: frozen dataclasses, tuples, `types.MappingProxyType` for dicts.
- If you need per-test mutability, switch to the default `per="test"` so each test gets a fresh value, at the cost of losing the caching benefit.
- For resources like database connections, use `yield` with explicit teardown so the resource's lifecycle is tied to the scope.

`per="test"` fixture values are created fresh per test and are not affected by this.
