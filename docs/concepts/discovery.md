# Test Discovery

Tryke discovers tests without running your code. It uses a Rust-powered Python parser
([Ruff](https://github.com/astral-sh/ruff)) to read your source files at startup, build
an import graph, and identify every test function.

## Recursion into structured blocks

Beyond scanning the module body, Tryke recurses into two specific block shapes where
tests legitimately nest:

- `with describe("name"):` — tests inside are collected with the group name as a
  prefix. See [writing tests](../guides/writing-tests.md#grouping-tests-with-describe).
- `if __TRYKE_TESTING__:` — the in-source testing guard. Tests, fixtures, doctests,
  and nested `describe` blocks inside are discovered identically to module-level code,
  and static imports inside contribute to the import graph. See
  [in-source testing](../guides/writing-tests.md#in-source-testing).

No other `if`/`for`/`while` bodies are descended: keeping discovery narrow means
"where is this test defined?" has an obvious answer.

## What static analysis can see

| Pattern | Tracked |
|---------|---------|
| `import foo` | ✅ |
| `from foo.bar import baz` | ✅ |
| `from . import utils` (relative) | ✅ |
| `from __future__ import annotations` | ✅ |
| `TYPE_CHECKING` imports | ✅ |
| `lazy import foo` / `lazy from foo import bar` (PEP 810) | ✅ |
| `__lazy_modules__ = ["sub"]` in a package `__init__.py` | ✅ |
| `importlib.import_module("foo")` | ❌ |
| `__import__("foo")` | ❌ |
| Tests defined in dynamically-loaded modules | ❌ |

## What happens when dynamic imports are detected

Tryke detects `importlib.import_module()` and `__import__()` calls during the parse
phase. When a file contains either, Tryke marks it as **always re-run**.

### Effect on `--changed` and `--changed-first` mode

The file is included in every `--changed` invocation, regardless of whether anything it
statically imports changed. A single widely-imported helper with one dynamic import can
pull many tests back into every run — silently undermining the precision of
impact-based filtering.

Tryke will emit a warning when this happens:

```text
warning: tests/helpers/loader.py — dynamic imports found; this file will always re-run with --changed
         replace importlib.import_module() or __import__() with static imports to restore selective re-runs
```

### Effect on watch and server mode

Watch and server mode pick up code changes by restarting the worker subprocesses
between cycles, so every cycle starts with an empty `sys.modules`. Both static
and dynamic imports load the latest source on the first test that needs them —
there is no stale-cache pitfall here.

The static import graph still drives **test selection**: only tests that
transitively import a changed file are rerun. A file with a dynamic import is
marked always-dirty (the same rule as `--changed`), so any test that imports it
will rerun on every change. To narrow watch reruns, prefer static imports — see
the recommendations below.

## Recommendations

### Prefer static imports

The most direct fix is to replace dynamic calls with static imports wherever the module
name is known at write time:

```python
# before
mod = importlib.import_module("myapp.plugins.csv_plugin")

# after
from myapp.plugins import csv_plugin
```

### Isolate dynamic loading from test code

If you genuinely need runtime module selection (plugin systems, feature flags), keep the
dynamic logic in non-test production code and test through a static interface:

```python
# helpers/loader.py — dynamic loading lives here
def load_plugin(name: str):
    return importlib.import_module(f"myapp.plugins.{name}")

# tests/test_loader.py — only static imports here
from helpers.loader import load_plugin

@test
def loads_csv_plugin():
    plugin = load_plugin("csv_plugin")
    expect(plugin).not_.to_be_none()
```

This way only `helpers/loader.py` is marked always-dirty. Tests that don't import it
are unaffected.

### Use `TYPE_CHECKING` for type-only imports

Tryke already handles `TYPE_CHECKING` blocks correctly — they are treated as static
imports and fully tracked:

```python
from __future__ import annotations
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from myapp import HeavyType  # tracked, not flagged
```

### Use `lazy import` for deferred imports (PEP 810)

Python 3.15 introduces an explicit `lazy` keyword that defers a real import to the
first use of the bound name. Tryke parses `lazy import` and `lazy from` exactly like
their eager counterparts and adds them to the import graph, so deferring an import
does **not** break `--changed` selection or watch-mode reruns:

```python
# tracked just like a plain import
lazy import heavy_module
lazy from myapp import heavy_helper
```

Tryke also recognises the PEP 810 transitional `__lazy_modules__` declaration in a
package's `__init__.py`. Each listed submodule is treated as a static dependency of
the package, so editing a lazily-exposed submodule still re-runs tests that touch
the package:

```python
# myapp/__init__.py
__lazy_modules__ = ["plugins", "heavy_helpers"]
```

### Exclude files that don't contain tests

If a file uses dynamic imports for non-test purposes (code generation, benchmarks,
scripts) and contains no tests, tell Tryke to ignore it:

```toml
# pyproject.toml
[tool.tryke]
exclude = ["scripts/generate.py", "benchmarks/suites"]
```

### Accept the tradeoff when it's intentional

If a file intentionally tests dynamic loading behavior — for example, a test that
verifies your plugin loader works — that's fine. Just be aware that those tests
will always run with `--changed`, since the import graph cannot prove they aren't
affected by an unrelated change. (Watch and server mode restart workers on every
change, so the dynamically-loaded code itself is always reloaded fresh.)
