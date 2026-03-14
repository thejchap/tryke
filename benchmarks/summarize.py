# /// script
# requires-python = ">=3.12"
# ///
"""Generate benchmark summaries and embed them into the docs.

Usage:
    uv run python benchmarks/summarize.py
    uv run python benchmarks/summarize.py --check
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
RESULTS_DIR = Path(__file__).parent / "results"
RESULTS_OUTPUT = Path(__file__).parent / "RESULTS.md"
DOCS_OUTPUT = ROOT / "docs" / "benchmarks.md"
DOCS_START_MARKER = "<!-- BENCHMARKS:START -->"
DOCS_END_MARKER = "<!-- BENCHMARKS:END -->"

SCALES = (50, 500, 5000)
_MIN_RESULTS = 2
_MICROSECOND_THRESHOLD = 0.001


def _load_json(path: Path) -> dict[str, Any] | None:
    if not path.exists():
        return None
    with path.open(encoding="utf-8") as file:
        data = json.load(file)
    if not isinstance(data, dict):
        msg = f"expected JSON object in {path}"
        raise TypeError(msg)
    return data


def _load_result(name: str, results_dir: Path) -> dict[str, Any] | None:
    return _load_json(results_dir / f"{name}.json")


def _fmt(seconds: float) -> str:
    if seconds < _MICROSECOND_THRESHOLD:
        return f"{seconds * 1_000_000:.0f}\u00b5s"
    if seconds < 1:
        return f"{seconds * 1000:.1f}ms"
    return f"{seconds:.2f}s"


def _ratio(a: float, b: float) -> str:
    if b == 0:
        return "\u2014"
    return f"{a / b:.1f}x"


def _append_table(
    lines: list[str],
    *,
    title: str,
    headers: tuple[str, ...],
    stem: str,
    results_dir: Path,
) -> None:
    lines.append(title)
    lines.append("| " + " | ".join(headers) + " |")
    separator = "| " + " | ".join("-" * len(header) for header in headers) + " |"
    lines.append(separator)

    for scale in SCALES:
        payload = _load_result(f"{stem}_{scale}", results_dir)
        results = payload.get("results") if payload else None
        if not isinstance(results, list) or len(results) < _MIN_RESULTS:
            continue

        first = results[0]
        second = results[1]
        if not isinstance(first, dict) or not isinstance(second, dict):
            continue

        try:
            tryke_mean = float(first["mean"])
            pytest_mean = float(second["mean"])
        except (KeyError, TypeError, ValueError):
            continue

        speedup = _ratio(pytest_mean, tryke_mean)
        lines.append(
            f"| {scale} | {_fmt(tryke_mean)} | {_fmt(pytest_mean)} | {speedup} |"
        )

    lines.append("")


def _metadata_line(label: str, value: str | None) -> str:
    return f"- **{label}:** {value or 'unknown'}"


def _render_environment(metadata: dict[str, Any] | None) -> list[str]:
    lines = ["## Benchmark Environment", ""]
    if metadata is None:
        lines.append("_System metadata unavailable for these benchmark results._")
        lines.append("")
        return lines

    platform = metadata.get("platform")
    cpu = metadata.get("cpu")
    versions = metadata.get("versions")
    benchmark = metadata.get("benchmark")

    platform_name = None
    platform_release = None
    architecture = None
    if isinstance(platform, dict):
        platform_name = platform.get("system")
        platform_release = platform.get("release")
        architecture = platform.get("architecture")

    cpu_model = None
    logical_cores = None
    if isinstance(cpu, dict):
        cpu_model = cpu.get("model")
        logical_cores = cpu.get("logical_cores")

    python_version = None
    hyperfine_version = None
    tryke_version = None
    pytest_version = None
    pytest_xdist_version = None
    if isinstance(versions, dict):
        python_version = versions.get("python")
        hyperfine_version = versions.get("hyperfine")
        tryke_version = versions.get("tryke")
        pytest_version = versions.get("pytest")
        pytest_xdist_version = versions.get("pytest_xdist")

    warmup = min_runs = generated_at = None
    if isinstance(benchmark, dict):
        warmup = benchmark.get("warmup")
        min_runs = benchmark.get("min_runs")
        generated_at = benchmark.get("generated_at")

    os_value = (
        " ".join(part for part in [platform_name, platform_release] if part) or None
    )
    cpu_value = (
        " ".join(
            part
            for part in [
                cpu_model,
                f"({logical_cores} logical cores)" if logical_cores else None,
            ]
            if part
        )
        or None
    )
    benchmark_value = (
        " / ".join(
            part
            for part in [
                f"warmup {warmup}" if warmup is not None else None,
                f"min-runs {min_runs}" if min_runs is not None else None,
            ]
            if part
        )
        or None
    )

    lines.extend(
        [
            _metadata_line("Generated", generated_at),
            _metadata_line("OS", os_value),
            _metadata_line("Architecture", architecture),
            _metadata_line("CPU", cpu_value),
            _metadata_line("Python", python_version),
            _metadata_line("tryke", tryke_version),
            _metadata_line("pytest", pytest_version),
            _metadata_line("pytest-xdist", pytest_xdist_version),
            _metadata_line("hyperfine", hyperfine_version),
            _metadata_line("Benchmark params", benchmark_value),
            "",
        ]
    )
    return lines


def render_results_sections(results_dir: Path = RESULTS_DIR) -> str:
    metadata = _load_json(results_dir / "system.json")
    lines = [
        "> Auto-generated by `uv run python benchmarks/summarize.py`.",
        "> Do not edit manually.",
        "",
    ]
    lines.extend(_render_environment(metadata))
    _append_table(
        lines,
        title="## Discovery",
        headers=("Scale", "tryke", "pytest", "Speedup"),
        stem="discovery",
        results_dir=results_dir,
    )
    _append_table(
        lines,
        title="## Sequential: tryke (`-j1`) vs pytest",
        headers=("Scale", "tryke -j1", "pytest", "Speedup"),
        stem="sequential",
        results_dir=results_dir,
    )
    _append_table(
        lines,
        title="## Parallel: tryke vs pytest-xdist (`-nauto`)",
        headers=("Scale", "tryke", "pytest-xdist", "Speedup"),
        stem="parallel",
        results_dir=results_dir,
    )
    return "\n".join(lines).rstrip() + "\n"


def render_results_markdown(results_dir: Path = RESULTS_DIR) -> str:
    return "# Benchmark Results\n\n" + render_results_sections(results_dir)


def update_docs_markdown(existing_docs: str, generated_section: str) -> str:
    start = existing_docs.find(DOCS_START_MARKER)
    end = existing_docs.find(DOCS_END_MARKER)
    if start == -1 or end == -1 or end < start:
        msg = "benchmark doc markers are missing from docs/benchmarks.md"
        raise ValueError(msg)

    before = existing_docs[: start + len(DOCS_START_MARKER)]
    after = existing_docs[end:]
    replacement = f"{before}\n\n{generated_section.rstrip()}\n\n"
    return replacement + after


def generate_outputs(
    *,
    results_dir: Path = RESULTS_DIR,
    docs_path: Path = DOCS_OUTPUT,
) -> dict[Path, str]:
    generated_section = render_results_sections(results_dir)
    docs_source = docs_path.read_text(encoding="utf-8")
    docs_output = update_docs_markdown(docs_source, generated_section)
    return {
        RESULTS_OUTPUT: render_results_markdown(results_dir),
        docs_path: docs_output,
    }


def _write_or_check(path: Path, content: str, *, check: bool) -> bool:
    existing = path.read_text(encoding="utf-8") if path.exists() else None
    if existing == content:
        return False
    if check:
        return True
    path.write_text(content, encoding="utf-8")
    return True


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check", action="store_true", help="fail if generated outputs are out of date"
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    changed_paths: list[Path] = []
    for path, content in generate_outputs().items():
        if _write_or_check(path, content, check=args.check):
            changed_paths.append(path)

    if args.check:
        if changed_paths:
            for path in changed_paths:
                sys.stderr.write(f"generated benchmark docs are out of date: {path}\n")
            return 1
        sys.stdout.write("benchmark docs are up to date\n")
        return 0

    for path in changed_paths:
        sys.stdout.write(f"wrote {path}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
