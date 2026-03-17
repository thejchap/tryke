# Changed mode

Changed mode uses git to detect which files have been modified and runs only the tests affected by those changes.

## `--changed`

Run only tests affected by uncommitted changes:

```bash
tryke test --changed
```

This compares the working tree against `HEAD`, finds changed `.py` files, then uses the import graph to identify every test that transitively depends on a changed module.

## `--changed-first`

Run affected tests first, then run the rest of the suite:

```bash
tryke test --changed-first
```

This gives you fast feedback on what you just changed while still verifying the full suite in the same run.

## `--base-branch`

Compare against a branch instead of `HEAD`:

```bash
tryke test --changed --base-branch main
```

This uses `git merge-base` to find the common ancestor and diffs from there. Useful in CI to run only tests affected by a pull request:

```bash
tryke test --changed --base-branch origin/main
```

`--base-branch` works with both `--changed` and `--changed-first`.

## How it works

1. tryke runs `git diff` to find changed `.py` files
2. The static import graph (see [test discovery](../concepts/discovery.md)) maps each changed file to the test files that depend on it
3. Only the affected test files are collected and run

Files with dynamic imports are always included — see [discovery](../concepts/discovery.md#what-happens-when-dynamic-imports-are-detected).

## Combining with other filters

Changed mode composes with all other flags:

```bash
# Only changed tests matching a name pattern
tryke test --changed -k "parse"

# Only changed tests with a specific tag
tryke test --changed -m "unit"

# Changed-first with fail-fast
tryke test --changed-first -x
```

## Visualizing the impact

Use `tryke graph --changed` to see which files are affected without running any tests:

```bash
tryke graph --changed
tryke graph --changed --base-branch main
```
