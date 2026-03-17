# tryke

tryke is a fast, modern test framework for Python.

[![Ruff](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/astral-sh/ruff/main/assets/badge/v2.json)](https://github.com/astral-sh/ruff)
[![PyPI](https://img.shields.io/pypi/v/tryke)](https://pypi.org/project/tryke/)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![python](https://img.shields.io/badge/python-3.12%20%7C%203.13%20%7C%203.14-blue.svg)](https://python.org)
[![CI](https://github.com/thejchap/tryke/actions/workflows/ci.yml/badge.svg)](https://github.com/thejchap/tryke/actions/workflows/ci.yml)
[![docs](https://img.shields.io/badge/docs-thejchap.github.io%2Ftryke-blue)](https://thejchap.github.io/tryke/)

<img width="800" height="442" alt="Screenshot 2026-03-16 at 23 01 52" src="https://github.com/user-attachments/assets/ea075157-3555-429d-b230-54ecf94d656a" />

## Getting started

For more information, see the [documentation](https://thejchap.github.io/tryke/).

Write a test.

```python
from tryke import expect, test, describe

def add(a: int, b: int) -> int:
    return a + b

with describe("add"):
    @test("1 + 1")
    def test_basic():
        expect(1 + 1).to_equal(2)
```

Run your tests.

```bash
uvx tryke test
```

## License

This repository is licensed under the MIT License.
