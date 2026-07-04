# Editor integrations

Editor plugins launch `tryke server` as a child process and communicate with it over the child's stdin/stdout using newline-delimited JSON-RPC 2.0 — see [client/server mode](../concepts/client-server.md). (This replaces the earlier TCP model where plugins connected to `127.0.0.1:<port>`.)

## Neovim

Neovim support is provided by a Neotest plugin: [neotest-tryke](https://github.com/thejchap/neotest-tryke).

## VS Code

VS Code support is provided by the [tryke-vscode](https://github.com/thejchap/tryke-vscode) plugin.
