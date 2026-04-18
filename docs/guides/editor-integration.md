# Editor integrations

## Neovim

Neovim support is provided by a Neotest plugin: [neotest-tryke](https://github.com/thejchap/neotest-tryke).

## VS Code

VS Code support is provided by the [tryke-vscode](https://github.com/thejchap/tryke-vscode) plugin.

## In-source tests

Both plugins support [in-source tests](writing-tests.md#in-source-testing) unchanged —
they consume Tryke's discovery output, which already carries guard-nested tests with
correct file and line information.
