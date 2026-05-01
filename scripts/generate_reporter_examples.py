# /// script
# requires-python = ">=3.12"
# ///
"""
Generate sample output for each tryke reporter and embed into
``docs/guides/reporters.md`` between ``<!-- REPORTER:NAME:START/END -->``
markers.

Each reporter is invoked twice from ``demo/`` so the second run hits a
warm discovery cache; only the second run's stdout is recorded. ANSI is
forced on (``FORCE_COLOR=1``) so the ``ansi`` code-fence picks up colors
when the docs are rendered with ``pygments-ansi-color``.

Usage:
    uv run python scripts/generate_reporter_examples.py
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEMO_DIR = ROOT / "demo"
SAMPLE = "sample.py"
DOCS_PATH = ROOT / "docs" / "guides" / "reporters.md"
INDEX_PATH = ROOT / "docs" / "index.md"

# (cli reporter name, code-fence language, extra cli flags)
REPORTERS: tuple[tuple[str, str, list[str]], ...] = (
    ("text", "ansi", ["-v"]),
    ("dot", "ansi", []),
    ("json", "json", []),
    ("junit", "xml", []),
    ("llm", "text", []),
    ("next", "ansi", []),
    ("sugar", "ansi", []),
)


# Hardcoded illustrative timings sampled from one real warm-cache run
# (text reporter, sample.py, second invocation so the discovery cache
# is hot). These get spliced in after capture so the docs render real
# values without the pre-commit hook rewriting reporters.md on every
# invocation as wall-clock timings drift.
_RUN_MS = "36.36"
_DISCOVER_MS = "0.76"
_TESTS_MS = "35.60"
_RUN_SECS_FRACTION = "0.036"
_CLOCK = "10:02:24"
_RUN_NANOS = "36360000"
_DISCOVER_NANOS = "760000"
_TESTS_NANOS = "35600000"

# Permits ANSI escapes inside `[^\d\n]`-style runs, which otherwise stop
# at the digits embedded in escape sequences like `\x1b[2m`.
_ANSI = r"(?:\x1b\[[\d;]*[a-zA-Z])"
_NON_DIGIT = rf"(?:{_ANSI}|[^\d\n])"


def _normalize(text: str) -> str:
    # Per-test bracket timings: tests round to 0 for our tiny sample, so
    # leaving them at 0.00ms / 0.000s is realistic.
    text = re.sub(r"(\[)\s*\d+(?:\.\d+)?ms(?!\w)(\s*\])", r"\g<1>0.00ms\g<2>", text)
    text = re.sub(r"(\[\s*)\d+\.\d+s(?!\w)(\s*\])", r"\g<1>0.000s\g<2>", text)

    # `Duration  X.XXms (discover X.XXms, tests X.XXms)` summary line.
    text = re.sub(
        rf"(Duration{_NON_DIGIT}*?)"
        rf"\d+(?:\.\d+)?ms({_NON_DIGIT}*?discover{_NON_DIGIT}*?)"
        rf"\d+(?:\.\d+)?ms({_NON_DIGIT}*?tests{_NON_DIGIT}*?)"
        r"\d+(?:\.\d+)?ms",
        rf"\g<1>{_RUN_MS}ms\g<2>{_DISCOVER_MS}ms\g<3>{_TESTS_MS}ms",
        text,
    )

    # LLM reporter: `<n> passed [X.XXms]` — promote the bracket value back
    # to the run total since it represents the whole run, not a single test.
    text = re.sub(
        r"(^|\n)(\d+ \w+ \[)\d+(?:\.\d+)?ms(\])",
        rf"\g<1>\g<2>{_RUN_MS}ms\g<3>",
        text,
    )

    # `Start at  HH:MM:SS` summary line.
    text = re.sub(
        rf"(Start at{_NON_DIGIT}*)\d{{2}}:\d{{2}}:\d{{2}}",
        rf"\g<1>{_CLOCK}",
        text,
    )

    # JSON: `"start_time":"HH:MM:SS"`
    text = re.sub(
        r'("start_time":")\d{2}:\d{2}:\d{2}',
        rf"\g<1>{_CLOCK}",
        text,
    )
    # JSON: discovery_duration / test_duration on the run summary.
    text = re.sub(
        r'("discovery_duration":\{"secs":)\d+(,"nanos":)\d+',
        rf"\g<1>0\g<2>{_DISCOVER_NANOS}",
        text,
    )
    text = re.sub(
        r'("test_duration":\{"secs":)\d+(,"nanos":)\d+',
        rf"\g<1>0\g<2>{_TESTS_NANOS}",
        text,
    )
    # JSON: top-level `"duration"` on the run summary block.
    text = re.sub(
        r'("summary":\{[^{}]*?"duration":\{"secs":)\d+(,"nanos":)\d+',
        rf"\g<1>0\g<2>{_RUN_NANOS}",
        text,
    )
    # JSON: per-test `"duration":{"secs":N,"nanos":N}` collapse to zero.
    text = re.sub(
        r'("duration":\{"secs":)\d+(,"nanos":)\d+',
        r"\g<1>0\g<2>0",
        text,
    )

    # JUnit: testsuite time first, testcases zero after.
    seen = [False]

    def _junit_time(_match: re.Match[str]) -> str:
        if seen[0]:
            return 'time="0.000"'
        seen[0] = True
        return f'time="{_RUN_SECS_FRACTION}"'

    return re.sub(r'time="\d+(?:\.\d+)?"', _junit_time, text)


def _run(reporter: str, extra: list[str]) -> str:
    env = os.environ.copy()
    env["FORCE_COLOR"] = "1"
    env.pop("NO_COLOR", None)
    cmd = [
        "uv",
        "run",
        "--project",
        str(ROOT),
        "tryke",
        "test",
        SAMPLE,
        "--reporter",
        reporter,
        "--no-progress",
        *extra,
    ]
    captured = ""
    for _ in range(2):
        result = subprocess.run(  # noqa: S603
            cmd,
            cwd=DEMO_DIR,
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )
        captured = result.stdout
    return _normalize(captured)


def _splice(content: str, name: str, lang: str, body: str) -> str:
    start = f"<!-- REPORTER:{name}:START -->"
    end = f"<!-- REPORTER:{name}:END -->"
    pattern = rf"{re.escape(start)}.*?{re.escape(end)}"
    block = f"```{lang}\n{body.rstrip()}\n```"
    replacement = f"{start}\n\n{block}\n\n{end}"
    if not re.search(pattern, content, flags=re.DOTALL):
        sys.stderr.write(f"missing markers for reporter {name!r} in {DOCS_PATH}\n")
        sys.exit(1)
    return re.sub(pattern, replacement, content, count=1, flags=re.DOTALL)


def main() -> int:
    bodies: dict[str, tuple[str, str]] = {}
    for name, lang, extra in REPORTERS:
        bodies[name] = (lang, _run(name, extra))

    for path, names in (
        (DOCS_PATH, [name for name, *_ in REPORTERS]),
        # The landing page only embeds the headline `text` reporter sample.
        (INDEX_PATH, ["text"]),
    ):
        original = path.read_text(encoding="utf-8")
        updated = original
        for name in names:
            lang, body = bodies[name]
            updated = _splice(updated, name, lang, body)
        if updated != original:
            path.write_text(updated, encoding="utf-8")
            sys.stdout.write(f"wrote {path}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
