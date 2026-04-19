# Installation

## Requirements

Tryke requires **Python 3.12 or newer**. Wheels are published for Linux (x86_64), macOS (arm64), and Windows (x86_64).

## Running without installation

The easiest way to get started with Tryke is with [uvx](https://docs.astral.sh/uv/guides/tools/):

```bash
uvx tryke test
```

Or with [pipx](https://pipx.pypa.io/):

```bash
pipx run tryke test
```

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
tryke test
```

To upgrade:

```bash
pip install --upgrade tryke
```

## Using Tryke in your editor

See the [editor integration guide](editor-integration.md)
