# Filtering tests

tryke supports three ways to narrow which tests run: name expressions, tag expressions, and path targeting. They can be combined freely.

## Name filtering with `-k`

The `-k` flag matches against test names using substring matching and boolean operators:

```bash
# Run tests whose name contains "math"
tryke test -k "math"

# Boolean AND
tryke test -k "math and addition"

# Boolean OR
tryke test -k "math or string"

# Boolean NOT
tryke test -k "not slow"

# Parentheses for grouping
tryke test -k "(math or string) and not slow"
```

The expression matches against the full test name including any `describe()` group prefix.

## Tag filtering with `-m`

The `-m` flag filters by [tags](writing-tests.md#tags) set on the `@test` decorator:

```python
from tryke import expect, test

@test(tags=["slow", "network"])
def downloads_large_file():
    ...

@test(tags=["fast"])
def adds_numbers():
    ...
```

```bash
# Run only tests tagged "slow"
tryke test -m "slow"

# Run tests tagged "fast" but not "network"
tryke test -m "fast and not network"

# Boolean OR
tryke test -m "slow or integration"
```

Tag expressions support the same `and`, `or`, `not`, and parentheses syntax as `-k`.

## Path targeting

Pass file or directory paths as positional arguments:

```bash
# Single file
tryke test tests/test_math.py

# Multiple paths
tryke test tests/unit/ tests/integration/

# Specific line
tryke test tests/test_math.py:12
```

The `file:line` syntax runs the test defined at that line. See [running tests](running-tests.md#fileline-syntax) for details.

## Combining filters

All filters are applied together. A test must satisfy every active filter to run:

```bash
# Tests in tests/unit/ whose name contains "parse" and that are tagged "fast"
tryke test tests/unit/ -k "parse" -m "fast"
```

## Verifying your filter

Use `--collect-only` to preview which tests match without running them:

```bash
tryke test -k "math and not slow" --collect-only
```
