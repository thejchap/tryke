# Installation

## Running without installation

The easiest way to get started with tryke is with [uvx](https://docs.astral.sh/uv/guides/tools/)

```bash
uvx tryke test
```

## Installation methods

### Adding tryke to your project

Use uv or your package manager of choice to add tryke as a dev dependency.

```bash
uv add --dev tryke
```

Then, use uv run to invoke ty:

```bash
uv run tryke
```

To update tryke, use --upgrade-package:

```bash
uv lock --upgrade-package tryke
```

## Using tryke in your editor

See the [editor integration guide](editor-integration.md)
