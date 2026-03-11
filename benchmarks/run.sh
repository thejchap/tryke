#!/usr/bin/env bash
set -euo pipefail

# Benchmark runner for tryke vs pytest
# Both runners execute the same test files — each test uses @test (tryke)
# and test_ prefix (pytest) with tryke's expect() assertions.
#
# Prerequisites: hyperfine, tryke (built), pytest + pytest-asyncio + pytest-xdist (via uv)
#
# Usage:
#   ./benchmarks/run.sh
#   WARMUP=1 MIN_RUNS=3 ./benchmarks/run.sh

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RESULTS_DIR="$SCRIPT_DIR/results"
SYSTEM_INFO_PATH="$RESULTS_DIR/system.json"

# Use relative paths — tryke discovers tests via project-root-relative walks,
# and absolute paths outside the walk root silently collect 0 tests.
SUITES_DIR="benchmarks/suites"

WARMUP="${WARMUP:-2}"
MIN_RUNS="${MIN_RUNS:-5}"
HYPERFINE_ARGS=("--warmup" "$WARMUP" "--min-runs" "$MIN_RUNS")

# ── helpers ──────────────────────────────────────────────────────────

die() { echo "error: $*" >&2; exit 1; }

require() {
    command -v "$1" >/dev/null 2>&1 || die "'$1' is required but not found"
}

capture_system_info() {
    RESULTS_DIR="$RESULTS_DIR" \
    SYSTEM_INFO_PATH="$SYSTEM_INFO_PATH" \
    NUM_CPUS="$NUM_CPUS" \
    WARMUP="$WARMUP" \
    MIN_RUNS="$MIN_RUNS" \
    uv run python <<'PY'
from __future__ import annotations

import json
import os
import platform
import subprocess
from datetime import UTC, datetime
from pathlib import Path


def run(*args: str) -> str | None:
    try:
        result = subprocess.run(args, capture_output=True, check=True, text=True)
    except (FileNotFoundError, subprocess.CalledProcessError):
        return None
    return result.stdout.strip() or None


def package_version(name: str) -> str | None:
    try:
        from importlib import metadata

        return metadata.version(name)
    except metadata.PackageNotFoundError:
        return None


def os_release() -> str | None:
    if platform.system() == "Linux":
        release = platform.freedesktop_os_release()
        pretty_name = release.get("PRETTY_NAME")
        if pretty_name:
            return pretty_name
    return platform.release() or None


cpu_model = platform.processor() or None
if platform.system() == "Darwin":
    cpu_model = run("sysctl", "-n", "machdep.cpu.brand_string") or cpu_model
elif platform.system() == "Linux":
    cpu_model = run("sh", "-c", "awk -F: '/model name/ {print $2; exit}' /proc/cpuinfo") or cpu_model

payload = {
    "platform": {
        "system": platform.system() or None,
        "release": os_release(),
        "architecture": platform.machine() or None,
    },
    "cpu": {
        "model": cpu_model.strip() if cpu_model else None,
        "logical_cores": int(os.environ["NUM_CPUS"]),
    },
    "versions": {
        "python": platform.python_version(),
        "hyperfine": run("hyperfine", "--version"),
        "tryke": run("tryke", "--version"),
        "pytest": package_version("pytest"),
        "pytest_xdist": package_version("pytest-xdist"),
    },
    "benchmark": {
        "generated_at": datetime.now(UTC).isoformat(timespec="seconds"),
        "warmup": int(os.environ["WARMUP"]),
        "min_runs": int(os.environ["MIN_RUNS"]),
    },
}

output = Path(os.environ["SYSTEM_INFO_PATH"])
output.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
PY
}

# tryke PATHS arg expects individual files, not directories
suite_files() {
    local dir="$1"
    echo "$dir"/test_*.py
}

# ── preflight ────────────────────────────────────────────────────────

require hyperfine
require tryke
uv run pytest --version >/dev/null 2>&1 || die "'pytest' is required (install via: uv add --dev pytest pytest-asyncio pytest-xdist)"

# generate suites if needed
if [[ ! -d "$SUITES_DIR/suite_50" ]]; then
    echo "generating test suites..."
    uv run python benchmarks/generate.py
fi

mkdir -p "$RESULTS_DIR"

NUM_CPUS=$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4)
capture_system_info

echo "=== tryke vs pytest benchmark suite ==="
echo "    cpus: $NUM_CPUS"
echo "    same test files, both runners"
echo ""

# ── sequential: tryke -j1 vs pytest (default) ───────────────────────

echo "━━ sequential: tryke -j1 vs pytest ━━"
echo ""

for scale in 50 500 5000; do
    echo "── scale: $scale tests ──"
    suite_dir="$SUITES_DIR/suite_$scale"
    test_files=$(suite_files "$suite_dir")

    # discovery
    echo "  discovery:"
    hyperfine "${HYPERFINE_ARGS[@]}" \
        --export-json "$RESULTS_DIR/discovery_${scale}.json" \
        --command-name "tryke ($scale)" \
        "tryke test --include benchmarks/suites --collect-only $test_files" \
        --command-name "pytest ($scale)" \
        "uv run pytest --collect-only -q $suite_dir"

    # execution — single worker vs sequential pytest
    echo "  execution (sequential):"
    hyperfine "${HYPERFINE_ARGS[@]}" \
        --export-json "$RESULTS_DIR/sequential_${scale}.json" \
        --command-name "tryke -j1 ($scale)" \
        "tryke test --include benchmarks/suites -j1 $test_files" \
        --command-name "pytest ($scale)" \
        "uv run pytest -q $suite_dir"

    echo ""
done

# ── parallel: tryke (default) vs pytest-xdist ───────────────────────

echo "━━ parallel: tryke vs pytest-xdist ━━"
echo ""

for scale in 50 500 5000; do
    echo "── scale: $scale tests ──"
    suite_dir="$SUITES_DIR/suite_$scale"
    test_files=$(suite_files "$suite_dir")

    echo "  execution (parallel):"
    hyperfine "${HYPERFINE_ARGS[@]}" \
        --export-json "$RESULTS_DIR/parallel_${scale}.json" \
        --command-name "tryke ($scale)" \
        "tryke test --include benchmarks/suites $test_files" \
        --command-name "pytest-xdist -nauto ($scale)" \
        "uv run pytest -q -nauto $suite_dir"

    echo ""
done

# ── summary ──────────────────────────────────────────────────────────

echo "=== results saved to $RESULTS_DIR/ ==="
echo ""
echo "refreshing benchmark docs..."
uv run python benchmarks/summarize.py
