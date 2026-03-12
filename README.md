# tryke

a test framework for python that is modern, fast, and fun



[![Ruff](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/astral-sh/ruff/main/assets/badge/v2.json)](https://github.com/astral-sh/ruff)
[![PyPI](https://img.shields.io/pypi/v/tryke)](https://pypi.org/project/tryke/)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![python](https://img.shields.io/badge/python-3.10%20%7C%203.11%20%7C%203.12%20%7C%203.13%20%7C%203.14-blue.svg)](https://python.org)
[![CI](https://github.com/thejchap/tryke/actions/workflows/ci.yml/badge.svg)](https://github.com/thejchap/tryke/actions/workflows/ci.yml)
[![docs](https://img.shields.io/badge/docs-thejchap.github.io%2Ftryke-blue)](https://thejchap.github.io/tryke/)


<img width="1920" height="1080" alt="300shots_so" src="https://github.com/user-attachments/assets/b882039b-1638-4cf5-b511-7631fe355139" />


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
- ghostty progress bar integration
- TODO doctest support
- TODO fixtures/dependency injection of some kind

## install

```bash
# in a project
uv add tryke

# globally
uv tool install tryke@latest

# or run directly without installing
uvx tryke test
```

## benchmarks

Run benchmarks locally to compare tryke vs pytest across different scales:

```bash
uv run python benchmarks/generate.py   # generate synthetic test suites
./benchmarks/run.sh                    # run hyperfine benchmarks and refresh docs
uv run python benchmarks/summarize.py --check
```

See the [benchmarks documentation](https://thejchap.github.io/tryke/benchmarks/) for methodology and details.

## documentation

Full documentation is available at **[thejchap.github.io/tryke](https://thejchap.github.io/tryke/)**:

- [Quick Start](https://thejchap.github.io/tryke/quickstart/) — up and running in 2 minutes
- [API Reference](https://thejchap.github.io/tryke/api/) — full decorator and assertion API
- [Migration from pytest](https://thejchap.github.io/tryke/migration/) — side-by-side cheat sheet
- [Why tryke?](https://thejchap.github.io/tryke/why/) — honest comparison with pytest

## ide support

- [neotest](https://github.com/thejchap/neotest-tryke)
- [vscode](https://github.com/thejchap/tryke-vscode)
