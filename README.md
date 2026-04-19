<p align="center">
  <a href="https://tryke.dev">
    <img height="170" alt="tryke-small" src="https://github.com/user-attachments/assets/39a2521a-fe9a-4235-8bb8-97b9e4f68aa7" />
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
  <a href="https://tryke.dev/">
    <img src="https://img.shields.io/badge/docs-tryke.dev-blue" alt="docs" />
  </a>
</p>

<video src="https://private-user-images.githubusercontent.com/2475286/580446781-354e21a4-b49f-4e93-a052-df98c0dfc3ae.mp4?jwt=eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.eyJpc3MiOiJnaXRodWIuY29tIiwiYXVkIjoicmF3LmdpdGh1YnVzZXJjb250ZW50LmNvbSIsImtleSI6ImtleTUiLCJleHAiOjE3NzY2MDk5NTAsIm5iZiI6MTc3NjYwOTY1MCwicGF0aCI6Ii8yNDc1Mjg2LzU4MDQ0Njc4MS0zNTRlMjFhNC1iNDlmLTRlOTMtYTA1Mi1kZjk4YzBkZmMzYWUubXA0P1gtQW16LUFsZ29yaXRobT1BV1M0LUhNQUMtU0hBMjU2JlgtQW16LUNyZWRlbnRpYWw9QUtJQVZDT0RZTFNBNTNQUUs0WkElMkYyMDI2MDQxOSUyRnVzLWVhc3QtMSUyRnMzJTJGYXdzNF9yZXF1ZXN0JlgtQW16LURhdGU9MjAyNjA0MTlUMTQ0MDUwWiZYLUFtei1FeHBpcmVzPTMwMCZYLUFtei1TaWduYXR1cmU9MTM3YTFjYmRiZmE4ZDkyNWI3Njg1MmY2MTY4NjU4NWJmMTc5ODQ1NDYyMmFkMTA1NDEyOGY1ZGFlY2M1ODBmZiZYLUFtei1TaWduZWRIZWFkZXJzPWhvc3QmcmVzcG9uc2UtY29udGVudC10eXBlPXZpZGVvJTJGbXA0In0.fMHzL9MdP8hUyQFDlPtjLGrJ9uyEXBW3JKgtwcb6sD0" data-canonical-src="https://private-user-images.githubusercontent.com/2475286/580446781-354e21a4-b49f-4e93-a052-df98c0dfc3ae.mp4?jwt=eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.eyJpc3MiOiJnaXRodWIuY29tIiwiYXVkIjoicmF3LmdpdGh1YnVzZXJjb250ZW50LmNvbSIsImtleSI6ImtleTUiLCJleHAiOjE3NzY2MDk5NTAsIm5iZiI6MTc3NjYwOTY1MCwicGF0aCI6Ii8yNDc1Mjg2LzU4MDQ0Njc4MS0zNTRlMjFhNC1iNDlmLTRlOTMtYTA1Mi1kZjk4YzBkZmMzYWUubXA0P1gtQW16LUFsZ29yaXRobT1BV1M0LUhNQUMtU0hBMjU2JlgtQW16LUNyZWRlbnRpYWw9QUtJQVZDT0RZTFNBNTNQUUs0WkElMkYyMDI2MDQxOSUyRnVzLWVhc3QtMSUyRnMzJTJGYXdzNF9yZXF1ZXN0JlgtQW16LURhdGU9MjAyNjA0MTlUMTQ0MDUwWiZYLUFtei1FeHBpcmVzPTMwMCZYLUFtei1TaWduYXR1cmU9MTM3YTFjYmRiZmE4ZDkyNWI3Njg1MmY2MTY4NjU4NWJmMTc5ODQ1NDYyMmFkMTA1NDEyOGY1ZGFlY2M1ODBmZiZYLUFtei1TaWduZWRIZWFkZXJzPWhvc3QmcmVzcG9uc2UtY29udGVudC10eXBlPXZpZGVvJTJGbXA0In0.fMHzL9MdP8hUyQFDlPtjLGrJ9uyEXBW3JKgtwcb6sD0" controls="controls" muted="muted" class="d-block rounded-bottom-2 border-top width-fit" style="max-height:640px; min-height: 200px">

  </video>

## Getting started

For more information, see the [documentation](https://tryke.dev/).

Write a test.

```python
from tryke import expect, test, describe

def add(a: int, b: int) -> int:
    return a + b

with describe("add"):

    @test("1 + 1")
    def basic():
        expect(add(1, 1)).to_equal(2)
```

Run your tests.

```bash
uvx tryke test
```

## License

This repository is licensed under the MIT License.
