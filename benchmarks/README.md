# Benchmarks

Comparative benchmarks for tryke vs pytest across different test suite sizes.

## Prerequisites

- [hyperfine](https://github.com/sharkdp/hyperfine) — command-line benchmarking tool
- `tryke` — built and on PATH (`cargo build --release && cargo install --path crates/tryke`)
- `pytest` + `pytest-asyncio` — `uv pip install pytest pytest-asyncio`

## Usage

### Generate test suites

```bash
uv run python benchmarks/generate.py
```

This creates synthetic test files in `benchmarks/suites/` at 3 scales (50, 500, 5000 tests) for both tryke and pytest formats, covering sync, async, and mixed tests.

### Run benchmarks

```bash
./benchmarks/run.sh
```

This runs all benchmarks using hyperfine and saves JSON results to `benchmarks/results/`.
On success it also refreshes `benchmarks/RESULTS.md` and the generated benchmark section in
`docs/benchmarks.md`.

Pass extra arguments to hyperfine:

```bash
./benchmarks/run.sh --warmup 5 --min-runs 10
```

### Generate summary

```bash
uv run python benchmarks/summarize.py
```

Produces `benchmarks/RESULTS.md` and updates the generated results block in
`docs/benchmarks.md`.

Validate that generated benchmark docs are current:

```bash
uv run python benchmarks/summarize.py --check
```

## What's measured

| Metric | Description |
|--------|-------------|
| Discovery time | `--collect-only` — time to find and parse tests |
| Execution (standalone) | Cold-start `tryke test` vs `pytest` |
| Execution (server) | Warm `tryke test --port 2337` with persistent server |

## Scales

- **50 tests** — small project / single module
- **500 tests** — medium project
- **5000 tests** — large monorepo / CI bottleneck scenario
