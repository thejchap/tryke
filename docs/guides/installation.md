# Installation

## Requirements

Tryke requires **Python 3.12 or newer**. Wheels are published for Linux (x86_64), macOS (arm64), and Windows (x86_64).

## Running without installation

The easiest way to try Tryke without installing it is with [uvx](https://docs.astral.sh/uv/guides/tools/):

```bash
uvx tryke test
```

Or with [pipx](https://pipx.pypa.io/):

```bash
pipx run tryke test
```

These commands run the suite once from a temporary tool environment. After
adding Tryke to your project, run `tryke` or `uv run tryke` to start the
default watch loop.

## Installation methods

### Adding Tryke to your project (uv)

Use [uv](https://docs.astral.sh/uv/) to add Tryke as a dev dependency.

```bash
uv add --dev tryke
```

Then, use uv run to invoke Tryke:

```bash
uv run tryke
```

That starts the default watch loop. Use `uv run tryke test` for a one-shot run.

To update Tryke, use --upgrade-package:

```bash
uv lock --upgrade-package tryke
```

### Adding Tryke to your project (pip)

Install Tryke into your active virtual environment:

```bash
pip install tryke
```

Invoke it directly:

```bash
tryke
```

That starts the default watch loop. Use `tryke test` for a one-shot run.

To upgrade:

```bash
pip install --upgrade tryke
```

## Using Tryke in your editor

See the [editor integration guide](editor-integration.md)

## Migrating from pytest

If you are moving an existing pytest suite to Tryke, see the [migration guide](../migration.md). It includes a side-by-side cheat sheet and a copy-paste AI prompt that walks an assistant through a phased migration with discovery and results-parity gates.
