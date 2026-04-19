# Tryke

Tryke is a fast Python test runner with zero runtime dependencies, per-assertion
diagnostics, and a clean decorator-based API.

<p class="md-badges">
  <a href="https://github.com/astral-sh/ruff">
    <img src="https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/astral-sh/ruff/main/assets/badge/v2.json" alt="ruff" />
  </a>
  <a href="https://pypi.org/project/tryke/">
    <img src="https://img.shields.io/pypi/v/tryke" alt="PyPI" />
  </a>
  <a href="https://github.com/thejchap/tryke/blob/main/LICENSE">
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

## Highlights

<ul class="md-highlights">
  <li><span class="hl-icon hl-icon-watch"></span><a href="guides/watch-mode.html">Watch mode</a></li>
  <li><span class="hl-icon hl-icon-async"></span>Native <code>async</code> support</li>
  <li><span class="hl-icon hl-icon-fast"></span><a href="concepts/discovery.html">Fast</a> test discovery</li>
  <li><span class="hl-icon hl-icon-insource"></span><a href="guides/writing-tests.html#in-source-testing">In-source testing</a></li>
  <li><span class="hl-icon hl-icon-doctests"></span>Support for <a href="https://docs.python.org/3/library/doctest.html">doctests</a></li>
  <li><span class="hl-icon hl-icon-clientsrv"></span><a href="concepts/client-server.html">Client/server</a> mode for fast editor integrations</li>
  <li><span class="hl-icon hl-icon-pretty"></span>Pretty, per-assertion diagnostics</li>
  <li><span class="hl-icon hl-icon-filter"></span>Filtering and marks</li>
  <li><span class="hl-icon hl-icon-changed"></span><a href="guides/changed-mode.html">Changed mode</a> (like <a href="https://github.com/anapaulagomes/pytest-picked">pytest-picked</a>)</li>
  <li><span class="hl-icon hl-icon-concurrent"></span>Concurrent tests</li>
  <li><span class="hl-icon hl-icon-soft"></span><a href="concepts/soft-assertions.html">Soft assertions</a></li>
  <li><span class="hl-icon hl-icon-reporters"></span>JSON, JUnit, Dot, and LLM reporters</li>
</ul>

## Getting started

Write a test.

```python
from tryke import expect, test, describe


def add(a: int, b: int) -> int:
    return a + b


with describe("add"):

    @test("1 + 1")
    def basic():
        expect(1 + 1).to_equal(2)
```

Run your tests.

```bash
uvx tryke test
```

## Installation

See the [installation](guides/installation.md) documentation.
