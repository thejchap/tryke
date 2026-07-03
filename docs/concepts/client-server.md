# Client/server mode

Tryke can run as a persistent server that caches test discovery while using fresh Python worker processes for each logical run. A client — typically an editor plugin — spawns `tryke server` as a child process and speaks JSON-RPC over its stdin/stdout, the same model language servers use.

## Starting the server

```bash
tryke server
```

On startup, the server:

1. Prepares the worker pool
2. Runs initial test discovery
3. Starts watching the filesystem for changes

It then reads requests from stdin and writes responses and notifications to stdout. Closing stdin shuts the server down cleanly.

## Protocol

The server speaks JSON-RPC 2.0 over stdin/stdout with newline-delimited messages: one JSON object per line.

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
| `did_change` | Client-initiated change signal; refreshes discovery before the next `run` |
| `run` | Execute tests with optional filters, streams results |

### Streaming notifications

During a test run, the server emits notifications (no `id` field) interleaved with responses on stdout:

```json
{"jsonrpc": "2.0", "method": "run_start", "params": {"tests": [...]}}
{"jsonrpc": "2.0", "method": "test_complete", "params": {"result": {...}}}
{"jsonrpc": "2.0", "method": "run_complete", "params": {"summary": {...}}}
```

File changes trigger a `discover_complete` notification with the updated test list.

### Debugging by hand

Because the transport is plain stdio, you can poke the server from a shell:

```bash
printf '{"jsonrpc":"2.0","id":1,"method":"ping"}\n' | tryke server
```

The server answers on stdout and exits when the pipe closes.

## Filesystem watching

The server watches all `.py` files in the project (respecting `.gitignore`) with a 50ms debounce. When files change:

1. The batch is dedup'd against each path's last-seen `(mtime, size)` so editor tail events that don't actually change the file (metadata fsync, swap-file cleanup, format-on-save with identical output) are dropped before any work happens
2. The import graph is incrementally updated
3. A `discover_complete` notification is emitted

File changes only update discovery. Every `run` request starts fresh worker subprocesses regardless of whether source files changed, so repeated runs also re-execute import-time code in brand-new Python interpreters.

## Editor integration

Server mode is designed for editor plugins. Two official integrations exist:

- **Neovim**: [neotest-tryke](https://github.com/thejchap/neotest-tryke)
- **VS Code**: [tryke-vscode](https://github.com/thejchap/tryke-vscode)

A typical workflow:

1. The editor plugin spawns `tryke server` as a child process and owns its stdio
2. The plugin sends a `discover` request to populate the test explorer
3. When you run a test, the plugin sends a `run` request
4. Results stream back as `test_complete` notifications for real-time progress
5. File changes automatically update the test list via `discover_complete`
6. On editor exit, the plugin closes the server's stdin (or kills the child) to stop it

See the [editor integration guide](../guides/editor-integration.md) for setup instructions.

## Why server mode

Without the server, every `tryke test` invocation pays for Python startup and test discovery. With a long-lived server:

- **Every run is isolated** — fresh Python processes re-execute imports and cannot leak module state from an earlier run
- **Discovery is cached** — only changed files are re-scanned
