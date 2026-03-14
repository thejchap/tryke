from __future__ import annotations

import importlib.util
import json
from pathlib import Path
from tempfile import TemporaryDirectory
from typing import TYPE_CHECKING

from tryke import expect, test

if TYPE_CHECKING:
    from types import ModuleType


def _load_module() -> ModuleType:
    path = Path(__file__).resolve().parent.parent / "benchmarks" / "summarize.py"
    spec = importlib.util.spec_from_file_location("benchmark_summarize", path)
    if spec is None or spec.loader is None:
        msg = "failed to load benchmark summarizer"
        raise RuntimeError(msg)

    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _write_json(path: Path, payload: dict) -> None:
    path.write_text(json.dumps(payload), encoding="utf-8")


def _benchmark_payload(tryke_mean: float, pytest_mean: float) -> dict:
    return {
        "results": [
            {"mean": tryke_mean},
            {"mean": pytest_mean},
        ]
    }


@test(name="benchmark summarize embeds generated docs block")
def test_generate_outputs_updates_results_and_docs() -> None:
    summarize = _load_module()

    with TemporaryDirectory() as tmpdir:
        root = Path(tmpdir)
        results_dir = root / "results"
        results_dir.mkdir()

        for stem, values in {
            "discovery_50": (0.1748, 0.1996),
            "sequential_50": (0.2314, 0.2397),
            "parallel_50": (0.2901, 1.02),
        }.items():
            _write_json(results_dir / f"{stem}.json", _benchmark_payload(*values))

        _write_json(
            results_dir / "system.json",
            {
                "platform": {
                    "system": "Linux",
                    "release": "Ubuntu 24.04",
                    "architecture": "x86_64",
                },
                "cpu": {
                    "model": "Example CPU",
                    "logical_cores": 8,
                },
                "versions": {
                    "python": "3.13.2",
                    "hyperfine": "hyperfine 1.19.0",
                    "tryke": "tryke 0.1.0",
                    "pytest": "9.0.2",
                    "pytest_xdist": "3.8.0",
                },
                "benchmark": {
                    "generated_at": "2026-03-12T12:00:00+00:00",
                    "warmup": 2,
                    "min_runs": 5,
                },
            },
        )

        docs_path = root / "benchmarks.md"
        docs_path.write_text(
            f"# Benchmarks\n\n{summarize.DOCS_START_MARKER}\n_old_\n"
            f"{summarize.DOCS_END_MARKER}\n",
            encoding="utf-8",
        )

        outputs = summarize.generate_outputs(
            results_dir=results_dir, docs_path=docs_path
        )

        results_markdown = outputs[summarize.RESULTS_OUTPUT]
        docs_markdown = outputs[docs_path]

        expect(results_markdown).to_contain("# Benchmark Results")
        expect(results_markdown).to_contain("## Benchmark Environment")
        expect(results_markdown).to_contain("Example CPU (8 logical cores)")
        expect(results_markdown).to_contain("| 50 | 174.8ms | 199.6ms | 1.1x |")
        expect(docs_markdown).to_contain(summarize.DOCS_START_MARKER)
        expect(docs_markdown).to_contain("tryke 0.1.0")
        expect(docs_markdown).to_contain(summarize.DOCS_END_MARKER)


@test(name="benchmark summarize tolerates missing system metadata")
def test_render_results_sections_without_metadata() -> None:
    summarize = _load_module()

    with TemporaryDirectory() as tmpdir:
        results_dir = Path(tmpdir)
        _write_json(results_dir / "discovery_50.json", _benchmark_payload(0.05, 0.10))

        rendered = summarize.render_results_sections(results_dir)

        expect(rendered).to_contain("System metadata unavailable")
        expect(rendered).to_contain("| 50 | 50.0ms | 100.0ms | 2.0x |")


@test(name="benchmark summarize requires doc markers")
def test_update_docs_markdown_requires_markers() -> None:
    summarize = _load_module()

    try:
        summarize.update_docs_markdown("# Benchmarks\n", "generated")
    except ValueError as exc:
        expect(str(exc)).to_contain("markers")
    else:
        msg = "expected missing-marker error"
        raise AssertionError(msg)
