---
template: home.html
title: Home
description: A Rust-based Python test runner with a Jest-style API.
hide:
  - navigation
  - toc
---

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
    <img src="https://img.shields.io/badge/python-3.12%20%7C%203.13%20%7C%203.14%20%7C%203.15-blue.svg" alt="python" />
  </a>
  <a href="https://github.com/thejchap/tryke/actions/workflows/release.yml">
    <img src="https://github.com/thejchap/tryke/actions/workflows/release.yml/badge.svg" alt="CI" />
  </a>
  <a href="https://tryke.dev/">
    <img src="https://img.shields.io/badge/docs-tryke.dev-blue" alt="docs" />
  </a>
</p>

<div class="tryke-feature-grid">
  <article class="tryke-feature-card">
    <h2><span class="hl-icon hl-icon-fast"></span>Fast discovery</h2>
    <p>Let Rust find your Python tests quickly, with cached discovery built for tight feedback loops.</p>
    <a href="concepts/discovery.html">How discovery works <span aria-hidden="true">→</span></a>
  </article>

  <article class="tryke-feature-card">
    <h2><span class="hl-icon hl-icon-concurrent"></span>Concurrent by default</h2>
    <p>Run independent tests in parallel without opting into a separate plugin or changing your suite.</p>
    <a href="concepts/concurrency.html">Concurrency model <span aria-hidden="true">→</span></a>
  </article>

  <article class="tryke-feature-card">
    <h2><span class="hl-icon hl-icon-pretty"></span>Readable diagnostics</h2>
    <p>See precise, per-assertion failures that keep the expected value and the actual result in view.</p>
    <a href="guides/writing-tests.html#assertions-with-expect">Writing expectations <span aria-hidden="true">→</span></a>
  </article>

  <article class="tryke-feature-card">
    <h2><span class="hl-icon hl-icon-soft"></span>Soft assertions</h2>
    <p>Collect multiple assertion failures in one test so a single mismatch does not hide the rest.</p>
    <a href="concepts/soft-assertions.html">Soft assertions <span aria-hidden="true">→</span></a>
  </article>

  <article class="tryke-feature-card">
    <h2><span class="hl-icon hl-icon-async"></span>Native async support</h2>
    <p>Write <code>async</code> tests and fixtures directly. There is no event-loop plugin to configure.</p>
    <a href="guides/async.html">Testing async code <span aria-hidden="true">→</span></a>
  </article>

  <article class="tryke-feature-card">
    <h2><span class="hl-icon hl-icon-watch"></span>Fast feedback</h2>
    <p>Stay in watch mode while you work, or run only tests affected by the files that changed.</p>
    <a href="guides/watch-mode.html">Watch mode <span aria-hidden="true">→</span></a>
  </article>

  <article class="tryke-feature-card">
    <h2><span class="hl-icon hl-icon-fixtures"></span>Typed fixtures</h2>
    <p>Compose setup and teardown with fixtures and explicit, typed <code>Depends()</code> injection.</p>
    <a href="guides/writing-tests.html#fixtures">Using fixtures <span aria-hidden="true">→</span></a>
  </article>

  <article class="tryke-feature-card">
    <h2><span class="hl-icon hl-icon-cases"></span>Expressive test structure</h2>
    <p>Parametrize with <code>@test.cases</code>, group with <code>describe()</code>, and mark test outcomes clearly.</p>
    <a href="concepts/cases.html">Parametrized tests <span aria-hidden="true">→</span></a>
  </article>

  <article class="tryke-feature-card">
    <h2><span class="hl-icon hl-icon-reporters"></span>Flexible reporting</h2>
    <p>Choose text, dot, JSON, JUnit, LLM, nextest-style, or pytest-sugar-style output for each workflow.</p>
    <a href="guides/reporters.html">Reporters <span aria-hidden="true">→</span></a>
  </article>

  <article class="tryke-feature-card">
    <h2><span class="hl-icon hl-icon-clientsrv"></span>Built for tooling</h2>
    <p>Use persistent client/server mode to power responsive editor integrations and other test clients.</p>
    <a href="concepts/client-server.html">Client/server mode <span aria-hidden="true">→</span></a>
  </article>
</div>

## Quick start

Run Tryke with [uvx](https://docs.astral.sh/uv/guides/tools/)—there is
nothing to install:

```bash
uvx tryke test
```

Leave off `test` to start watch mode:

```bash
uvx tryke
```

## A familiar test API

```python
from tryke import expect, test


@test
async def test_math():
    expect(40 + 2).to_equal(42)
```

Explore the [writing tests guide](guides/writing-tests.html), open the
[browser playground](https://playground.tryke.dev), or use the
[pytest migration guide](migration.html) for a side-by-side cheat sheet.
