# worker + scheduler design

## overview

test execution uses a pool of warm python worker processes that communicate
with the rust scheduler via json-rpc 2.0 over stdio.

workers are kept alive across runs during `server` and `watch` modes, and
respond to `reload` requests when test files change without restarting.

## crate: `tryke_runner`

provides `WorkerPool` and `WorkerProcess`.

### `WorkerPool`

- `new(size, python_bin)` — spawns `size` tokio tasks with lazy worker init
- `run(tests)` — distributes tests round-robin, returns `impl Stream<Item = TestResult>`
- `reload(modules)` — sends reload rpc to all workers, awaits acknowledgement
- `shutdown()` — signals all worker tasks to exit

### `WorkerProcess`

- `spawn(python_bin)` — starts `python3 -m tryke.worker` subprocess
- `run_test(test)` — sends `run_test` rpc, returns `TestResult`
- `reload(modules)` — sends `reload` rpc
- `ping()` — health check

workers are spawned lazily: the process is not started until the first message
arrives. if a worker process dies, it is respawned and the failed test retried once.

## python worker

`python/tryke/worker.py` — invoked as `python3 -m tryke.worker`

reads newline-delimited json-rpc 2.0 from stdin, writes responses to stdout.

methods:
- `ping` — returns "pong"
- `run_test {module, function}` — imports module lazily, runs function, captures stdout/stderr
- `reload {modules}` — calls `importlib.reload` for named modules

test outcomes: `passed`, `failed` (assertion error), `skipped` (unittest.SkipTest)

## protocol

request: `{"jsonrpc":"2.0","id":N,"method":"...","params":{...}}`

run_test response (passed):
```json
{"jsonrpc":"2.0","id":N,"result":{"outcome":"passed","duration_ms":5,"stdout":"","stderr":""}}
```

run_test response (failed):
```json
{"jsonrpc":"2.0","id":N,"result":{"outcome":"failed","message":"...","assertions":[],"duration_ms":5,"stdout":"","stderr":""}}
```

reload response: `{"jsonrpc":"2.0","id":N,"result":null}`

## integration

### cli (`tryke test`, `tryke watch`)

- creates `WorkerPool` once at startup
- `test`: streams results via `pool.run(tests)`, shuts pool down after
- `watch`: keeps pool alive, calls `pool.reload(modules)` on file change

### server (`tryke server`)

- `Server` creates `WorkerPool` in `run_on_listener`
- `ConnectionHandler` holds `Arc<WorkerPool>`
- `run` handler uses `pool.run(tests)` stream instead of fake results
- file watcher calls `pool.reload(modules)` before rediscovering

## module path conversion

file path → python module name:
- strip project root prefix
- strip `.py` suffix
- replace path separators with `.`

example: `/project/tests/test_math.py` → `tests.test_math`

implemented in `tryke_runner::path_to_module`.

## expect.py

`Expectation` methods now raise `AssertionError` with descriptive messages.
the worker catches `AssertionError` and maps it to a failed test result.
