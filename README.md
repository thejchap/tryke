<p align="center">
  <a href="https://thejchap.github.io/tryke">
    <img height="170" alt="tryke" src="https://github.com/user-attachments/assets/d5683277-642c-4a3c-bdfb-cbf4fdf99fe5" />
  </a>
</p>
<h1 align="center">Tryke</h1>

<p align="center">
  <a href="https://github.com/astral-sh/ruff">
    <img src="https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/astral-sh/ruff/main/assets/badge/v2.json" alt="ruff" />
  </a>
  <a href="https://pypi.org/project/tryke/">
    <img src="https://img.shields.io/pypi/v/tryke" alt="PyPI" />
  </a>
  <a href="LICENSE">
    <img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="license" />
  </a>
  <a href="https://python.org">
    <img src="https://img.shields.io/badge/python-3.12%20%7C%203.13%20%7C%203.14-blue.svg" alt="python" />
  </a>
  <a href="https://github.com/thejchap/tryke/actions/workflows/release.yml">
    <img src="https://github.com/thejchap/tryke/actions/workflows/release.yml/badge.svg" alt="CI" />
  </a>
  <a href="https://thejchap.github.io/tryke/">
    <img src="https://img.shields.io/badge/docs-thejchap.github.io%2Ftryke-blue" alt="docs" />
  </a>
</p>

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
        expect(add(1, 1)).to_equal(2)
```

Run your tests.

```bash
uvx tryke test
```

## License

This repository is licensed under the MIT License.
