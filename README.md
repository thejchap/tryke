# tryke

tryke is a fast, modern test framework for Python.

[![Ruff](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/astral-sh/ruff/main/assets/badge/v2.json)](https://github.com/astral-sh/ruff)
[![PyPI](https://img.shields.io/pypi/v/tryke)](https://pypi.org/project/tryke/)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![python](https://img.shields.io/badge/python-3.12%20%7C%203.13%20%7C%203.14-blue.svg)](https://python.org)
[![CI](https://github.com/thejchap/tryke/actions/workflows/ci.yml/badge.svg)](https://github.com/thejchap/tryke/actions/workflows/ci.yml)
[![docs](https://img.shields.io/badge/docs-thejchap.github.io%2Ftryke-blue)](https://thejchap.github.io/tryke/)
![300shots_so](https://github.com/user-attachments/assets/b882039b-1638-4cf5-b511-7631fe355139)

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
