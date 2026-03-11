# Benchmarks

Performance comparisons between tryke and pytest using synthetic test suites.

## Methodology

Benchmarks use [hyperfine](https://github.com/sharkdp/hyperfine) to measure wall-clock time across multiple runs with warmup iterations. Synthetic test suites are generated at 3 scales covering sync, async, and mixed test functions.

| Scale | Tests | Represents |
|-------|-------|------------|
| 50 | 150 (3 files × 50) | Small project |
| 500 | 1,500 (3 files × 500) | Medium project |
| 5,000 | 15,000 (3 files × 5,000) | Large monorepo |

## What's measured

### Discovery time

Time to find and parse all tests without executing them:

- **tryke:** `tryke test --collect-only`
- **pytest:** `pytest --collect-only -q`

Tryke discovers tests by parsing Python AST in Rust, without importing modules. Pytest imports modules to collect tests.

### Execution time (standalone)

Full cold-start execution:

- **tryke:** `tryke test`
- **pytest:** `pytest -q`

### Execution time (server mode)

With `tryke server` running persistently, `tryke test --port 2337` reuses cached discovery and persistent Python workers. This eliminates startup overhead on subsequent runs.

## Running benchmarks locally

```bash
# prerequisites
# install hyperfine: https://github.com/sharkdp/hyperfine
# install pytest: uv pip install pytest pytest-asyncio

# generate test suites
python benchmarks/generate.py

# run benchmarks
./benchmarks/run.sh

# generate markdown summary
python benchmarks/summarize.py
```

Results are saved to `benchmarks/results/` as JSON files. A markdown summary is generated at `benchmarks/RESULTS.md`.

## Why tryke is faster

1. **Rust binary startup** — no Python interpreter startup cost for the runner itself
2. **AST-based discovery** — tests are found by parsing source files in Rust, not by importing Python modules
3. **Concurrent execution** — tests run across multiple worker processes by default
4. **Server mode** — persistent workers and cached discovery eliminate repeated startup costs

The speed advantage is most pronounced at larger scales, where pytest's import-based discovery and plugin loading overhead grows linearly with test count.
