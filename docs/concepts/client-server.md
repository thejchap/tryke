# Client/server mode

tryke can run as a persistent server that keeps Python workers warm and caches test discovery. Clients connect over TCP to request test runs with minimal startup overhead.

## Starting the server

```bash
tryke server
```

The server listens on `127.0.0.1:2337` by default. Change the port with `--port`:

```bash
tryke server --port 9000
```

On startup, the server:

1. Spawns and pre-warms the worker pool
2. Runs initial test discovery
3. Starts watching the filesystem for changes

## Running tests against the server

Use `--port` on `tryke test` to connect to a running server instead of spawning fresh workers:

```bash
tryke test --port
tryke test --port 2337
```

All standard flags work: `-k`, `-m`, path arguments, `-x`, `--maxfail`. Filtering happens server-side.

```bash
tryke test --port -k "math" -x
```

## Protocol

The server uses JSON-RPC 2.0 over TCP with newline-delimited messages.

### Request/response

```json
{"jsonrpc": "2.0", "id": 1, "method": "ping"}
{"jsonrpc": "2.0", "id": 1, "result": "pong"}
```

### Available methods

| Method | Description |
|--------|-------------|
| `ping` | Liveness check, returns `"pong"` |
| `discover` | Re-scan for tests, returns the test list |
| `run` | Execute tests with optional filters, streams results |

### Streaming notifications

During a test run, the server broadcasts notifications (no `id` field) to all connected clients:

```json
{"jsonrpc": "2.0", "method": "run_start", "params": {"tests": [...]}}
{"jsonrpc": "2.0", "method": "test_complete", "params": {"result": {...}}}
{"jsonrpc": "2.0", "method": "run_complete", "params": {"summary": {...}}}
```

File changes trigger a `discover_complete` notification with the updated test list.

## Filesystem watching

The server watches all `.py` files in the project (respecting `.gitignore`) with a 200ms debounce. When files change:

1. The import graph is incrementally updated
2. Affected modules are reloaded in the worker pool
3. A `discover_complete` notification is broadcast to all connected clients

This means editors can keep their test explorer up to date in real time.

## Editor integration

Server mode is designed for editor plugins. Two official integrations exist:

- **Neovim**: [neotest-tryke](https://github.com/thejchap/neotest-tryke)
- **VS Code**: [tryke-vscode](https://github.com/thejchap/tryke-vscode)

A typical workflow:

1. Start the server in a terminal: `tryke server`
2. The editor plugin connects and sends a `discover` request to populate the test explorer
3. When you run a test, the plugin sends a `run` request
4. Results stream back as `test_complete` notifications for real-time progress
5. File changes automatically update the test list via `discover_complete`

See the [editor integration guide](../guides/editor-integration.md) for setup instructions.

## Why server mode

Without the server, every `tryke test` invocation pays for Python startup and test discovery. With the server:

- **Worker processes stay warm** — no Python startup per run
- **Discovery is cached** — only changed files are re-scanned
- **Multiple clients** — several editor windows or terminals can share one server
