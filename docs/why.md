# Why tryke?

An honest look at what tryke does well, where pytest wins, and when each tool is the right choice.

## What makes tryke different

### Rust-powered speed

Tryke's core — test discovery, scheduling, and reporting — is written in Rust. Python only runs the test functions themselves. This means startup, discovery, and result aggregation are fast regardless of project size.

### Per-assertion diagnostics

Every `expect()` call produces its own expected/received diagnostic. In a test with 5 assertions, you see exactly which ones passed and which failed — in a single run, without re-running or adding print statements.

### Zero dependencies

Tryke has no transitive Python dependencies. Your dependency tree stays clean.

### Modern API

A decorator-based API with chainable assertions:

```python
from tryke import expect, test

@test(name="user creation")
def create_user():
    user = create(name="alice")
    expect(user.name).to_equal("alice")
    expect(user.id).not_.to_be_none()
    expect(user.active).to_be_truthy()
```

### Built-in watch mode

`tryke watch` reruns tests on file changes — no plugins or extra tools needed.

### Client/server mode

`tryke server` keeps Python workers alive between runs. Subsequent `tryke test --port 2337` calls skip startup overhead, giving near-instant feedback during development.

### Changed-files mode

`tryke test --changed` uses git to determine which files changed and runs only the affected tests.

## Comparison

| | tryke | pytest |
|---|---|---|
| **Startup speed** | Fast (Rust binary) | Slower (Python + plugin loading) |
| **Discovery speed** | Fast (Rust AST parsing) | Slower (Python import) |
| **Execution** | Concurrent workers | Sequential (default) or xdist |
| **Diagnostics** | Per-assertion expected/received | Per-test with rewrite |
| **Dependencies** | Zero | Many transitive |
| **Watch mode** | Built-in | Plugin (pytest-watch) |
| **Server mode** | Built-in | Not available |
| **Changed files** | Built-in (`--changed`) | Plugin (pytest-picked) |
| **Async** | Built-in | Plugin (pytest-asyncio) |
| **Reporters** | text, json, dot, junit, llm | Verbose, short + plugins |
| **Plugin ecosystem** | — | Extensive (1000+) |
| **Fixtures** | Not yet | Powerful, composable |
| **Parametrize** | Not yet | Built-in |
| **Community** | New | Large, established |
| **Documentation** | Growing | Extensive |
| **IDE support** | VS Code, Neovim | All major IDEs |

## When to use tryke

- **New projects** where you don't need existing pytest plugins
- **Speed-sensitive CI** where test startup time matters
- **Developer experience** — rich diagnostics, watch mode, and server mode out of the box
- **Clean dependency trees** — tryke adds zero transitive dependencies
- **Async-heavy projects** — no extra plugins for async test support

## When to stick with pytest

- **Heavy plugin dependency** — if you rely on pytest-django, pytest-mock, factoryboy, or similar
- **Complex fixture graphs** — pytest's fixture system is unmatched
- **Stability requirements** — pytest is battle-tested across millions of projects
- **Team familiarity** — if your team knows pytest well and switching cost is high

## Roadmap

What's coming to tryke:

- **Fixtures / dependency injection** — a lightweight fixture system
- **Parametrize** — built-in test parameterization
- **Doctests** — run examples from docstrings as tests
- **Plugin API** — extensibility hooks for custom reporters and discovery
- **Coverage integration** — built-in coverage reporting
