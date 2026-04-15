# Graph

`tryke graph` prints dependency graphs that tryke builds during discovery — the import graph by default, and the fixture graph with `--fixtures`. These are the same graphs that power [changed mode](changed-mode.md) and fixture resolution, so exposing them directly is useful for debugging collection problems, exploring how files or fixtures are connected, and sanity-checking what `--changed` will actually run.

## Basic usage

```bash
tryke graph
```

Each entry shows a file, the modules it imports, and the modules that import it:

```text
calc/__init__.py
  imports:     calc/calc.py
  imported by: tests/test_async.py, tests/test_basics.py, tests/test_matchers.py

calc/calc.py
  imports:     (none)
  imported by: calc/__init__.py

tests/test_async.py
  imports:     calc/__init__.py
  imported by: (none)
```

The graph is static — it comes from parsing imports with Ruff, not from running your code — so it's fast and side-effect free. See [test discovery](../concepts/discovery.md).

## Options

### `--connected-only`

Hide files that have no imports and no importers. Handy when you want to focus on the wired-up part of the project:

```bash
tryke graph --connected-only
```

### `--changed`

Show only files affected by git-visible changes, labeling each entry as either `[changed]` (the file itself was modified) or `[affected]` (it transitively depends on a changed file). Requires git.

```bash
tryke graph --changed
```

```text
calc/calc.py [changed]
  imports:     (none)
  imported by: calc/__init__.py

calc/__init__.py [affected]
  imports:     calc/calc.py
  imported by: tests/test_async.py, tests/test_basics.py, tests/test_matchers.py
```

Use this to preview what `tryke test --changed` will run before running it.

### `--base-branch`

Compare against a branch instead of the working tree. Uses `git merge-base` to find the common ancestor.

```bash
tryke graph --changed --base-branch main
```

### `--fixtures`

Print the fixture (`@fixture` + `Depends()`) dependency graph instead of the import graph. Each entry shows a fixture's qualified name, its `per=test` / `per=scope` kind, the fixtures it depends on, and the fixtures (or tests) that depend on it:

```bash
tryke graph --fixtures
```

```text
tests.test_db::db  [per=scope]
  depends on:  (none)
  used by:     tests.test_db::session

tests.test_db::session  [per=test]
  depends on:  tests.test_db::db
  used by:     (none)
```

References to hooks tryke cannot resolve (typos, missing imports, dynamically-constructed names) are printed with a `?` suffix so you can spot them without running the suite. `--fixtures` is mutually exclusive with `--changed` / `--connected-only`.

### `--root`, `--exclude`, `--include`

Same semantics as the equivalent flags on `tryke test` — see [configuration](configuration.md).
