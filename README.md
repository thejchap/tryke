# tryke


[![Ruff](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/astral-sh/ruff/main/assets/badge/v2.json)](https://github.com/astral-sh/ruff)
[![PyPI](https://img.shields.io/pypi/v/tryke)](https://pypi.org/project/tryke/)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![python](https://img.shields.io/badge/python-3.12%20%7C%203.13-blue.svg)](https://python.org)
[![CI](https://github.com/thejchap/tryke/actions/workflows/ci.yml/badge.svg)](https://github.com/thejchap/tryke/actions/workflows/ci.yml)

a test framework for python that is modern, fast, and fun

<img width="610" height="294" alt="Screenshot 2026-03-08 at 00 07 31" src="https://github.com/user-attachments/assets/2920e023-d9bd-4906-b9ae-9b0a0d564600" />


## quickstart

```python
from tryke import expect, test

@test
def test_addition():
    expect(1 + 1).to_equal(2)
```

```bash
uvx tryke test
```

## features

- per-assertion diagnostic output
- in-source testing
- concurrent test execution
- watch mode with live reload
- changed-files mode (only run tests affected by git changes)
- filter by name (`-k`) or marker (`-m`)
- multiple reporters: text, json, junit, dot, llm
- llm reporter with compact output
- native async test support
- client/server mode

## expect api

```python
expect(value).to_equal(other)          # == equality
expect(value).to_be(other)             # `is` identity
expect(value).to_contain(item)         # `in` membership
expect(value).to_have_length(3)        # len() check
expect(value).to_match(r"\d+")         # regex search
expect(value).to_be_truthy()           # bool(value) is True
expect(value).to_be_none()             # value is None
expect(value).to_be_greater_than(0)    # > comparison

expect(fn).to_raise(ValueError, match="bad input")
```

Negate any matcher with `not_`:

```python
expect(result).not_.to_equal(0)
expect(items).not_.to_contain("secret")
```

Assertions are **soft by default** — multiple failures are collected
per test. Call `.fatal()` to stop immediately on failure:

```python
expect(config).to_be_truthy().fatal()  # stop here if None
expect(config["port"]).to_equal(8080)  # only runs if above passed
```

## test markers & tags

```python
@test(name="addition works", tags=["fast"])
def test_add():
    expect(1 + 1).to_equal(2)

@test.skip("flaky on CI")
def test_network(): ...

@test.skip_if(sys.platform == "win32", reason="unix only")
def test_permissions(): ...

@test.todo("not implemented yet")
def test_future_feature(): ...

@test.xfail("known upstream bug")
def test_broken_dep(): ...

@test
async def test_async_fetch():
    result = await fetch_data()
    expect(result).to_be_truthy()
```

## cli usage

```
tryke test                     # run all tests
tryke watch                    # re-run on file changes
tryke graph [--connected-only] # show import dependency graph
```

Key flags for `test` and `watch`:

```
-k FILTER          filter by name ("math and not slow")
-m MARKERS         filter by tag ("fast or unit")
--changed           only tests affected by git changes
-x, --fail-fast    stop after first failure
--maxfail N        stop after N failures
-j N               number of worker processes
--reporter FORMAT  text | json | junit | dot | llm
--collect-only     list discovered tests without running
-v / -vv / -q     verbose / very verbose / quiet
```

## install

```
uv add tryke
```

Or run directly without installing:

```
uvx tryke test
```

## ide support

- [neotest](https://github.com/thejchap/neotest-tryke)
- [vscode](https://github.com/thejchap/tryke-vscode)
