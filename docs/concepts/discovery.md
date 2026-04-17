# Test Discovery

tryke discovers tests without running your code. It uses a Rust-powered Python parser
([Ruff](https://github.com/astral-sh/ruff)) to read your source files at startup, build
an import graph, and identify every test function — all before a Python interpreter is
ever involved.

This is why tryke is fast: it reasons about your code the way a compiler does, at
parse time rather than runtime.

## Recursion into structured blocks

Beyond scanning the module body, tryke recurses into two specific block shapes where
tests legitimately nest:

- `with describe("name"):` — tests inside are collected with the group name as a
  prefix. See [writing tests](../guides/writing-tests.md#grouping-tests-with-describe).
- `if __TRYKE_TESTING__:` — the in-source testing guard. Tests, fixtures, doctests,
  and nested `describe` blocks inside are discovered identically to module-level code,
  and static imports inside contribute to the import graph. See
  [in-source testing](../guides/in-source-testing.md).

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
| `importlib.import_module("foo")` | ❌ |
| `__import__("foo")` | ❌ |
| Tests defined in dynamically-loaded modules | ❌ |

## What happens when dynamic imports are detected

tryke detects `importlib.import_module()` and `__import__()` calls during the parse
phase. When a file contains either, tryke marks it as **always re-run**.

### Effect on `--changed` and `--changed-first` mode

The file is included in every `--changed` invocation, regardless of whether anything it
statically imports changed. A single widely-imported helper with one dynamic import can
pull many tests back into every run — silently undermining the precision of
impact-based filtering.

tryke will emit a warning when this happens:

```text
warning: tests/helpers/loader.py — dynamic imports found; this file will always re-run with --changed
         replace importlib.import_module() or __import__() with static imports to restore selective re-runs
```

### Effect on watch and server mode

tryke tracks which Python modules to reload between test cycles by following static
import edges. Dynamic import edges are invisible to this graph. This means:

- If `helper.py` is only imported dynamically by `loader.py`, and `helper.py` changes,
  the watch-mode worker may not reload `helper` during that cycle.
- `loader.py` itself will always be reloaded (it is always-dirty), but when its code
  re-executes `importlib.import_module("helper")`, it may receive the stale cached
  version from `sys.modules`.
- This can cause tests to pass or fail based on code that hasn't been reflected yet,
  persisting until the watch session restarts.

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

tryke already handles `TYPE_CHECKING` blocks correctly — they are treated as static
imports and fully tracked:

```python
from __future__ import annotations
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from myapp import HeavyType  # tracked, not flagged
```

### Exclude files that don't contain tests

If a file uses dynamic imports for non-test purposes (code generation, benchmarks,
scripts) and contains no tests, tell tryke to ignore it:

```toml
# pyproject.toml
[tool.tryke]
exclude = ["scripts/generate.py", "benchmarks/suites"]
```

### Accept the tradeoff when it's intentional

If a file intentionally tests dynamic loading behavior — for example, a test that
verifies your plugin loader works — that's fine. Just be aware that:

- Those tests will always run with `--changed`
- In watch/server mode, editing the dynamically-loaded target may require restarting
  the watch session to pick up changes reliably
