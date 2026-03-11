# /// script
# requires-python = ">=3.12"
# ///
"""Generate synthetic test suites for benchmarking tryke vs pytest.

Produces a single shared suite that both runners can execute — each test
uses the @test decorator (for tryke) and the test_ prefix (for pytest),
with tryke's expect() assertions that also work under pytest since
ExpectationError extends AssertionError.

Usage:
    uv run python benchmarks/generate.py
"""

from __future__ import annotations

import sys
import textwrap
from pathlib import Path

SUITES_DIR = Path(__file__).parent / "suites"
SCALES = [50, 500, 5000]

# Each template is a test body that does realistic work.
# They are cycled across the generated tests so the suite
# contains a mix of workloads rather than one pattern repeated.
TEMPLATES = [
    # dict construction + iteration
    textwrap.dedent("""\
    data = {{f"key_{{j}}": list(range(j, j + 10)) for j in range({seed}, {seed} + 50)}}
    expect(len(data)).to_equal(50)
    total = sum(len(v) for v in data.values())
    expect(total).to_equal(500)
    """),
    # json roundtrip
    textwrap.dedent("""\
    import json
    rng = range({seed}, {seed} + 80)
    original = {{"items": [{{f"id_{{j}}": j * 1.1}} for j in rng]}}
    dumped = json.dumps(original, sort_keys=True)
    loaded = json.loads(dumped)
    expect(loaded).to_equal(original)
    expect(len(loaded["items"])).to_equal(80)
    """),
    # string sorting + joining
    textwrap.dedent("""\
    words = [f"word_{{j:04d}}" for j in range({seed}, {seed} + 500)]
    result = " ".join(sorted(words, reverse=True))
    expect(len(result.split())).to_equal(500)
    expect(result.split()[0]).to_be_greater_than(result.split()[-1])
    """),
    # hashing
    textwrap.dedent("""\
    import hashlib
    rng = range({seed}, {seed} + 150)
    digests = [hashlib.sha256(f"p_{{j}}".encode()).hexdigest() for j in rng]
    expect(len(digests)).to_equal(150)
    expect(len(set(digests))).to_equal(150)
    """),
    # list comprehension + filtering
    textwrap.dedent("""\
    nums = [j * j for j in range({seed}, {seed} + 300)]
    evens = [n for n in nums if n % 2 == 0]
    expect(len(nums)).to_equal(300)
    expect(all(n % 2 == 0 for n in evens)).to_be_truthy()
    """),
    # nested data structure
    textwrap.dedent("""\
    tree = {{}}
    for j in range({seed}, {seed} + 30):
        tree[f"node_{{j}}"] = {{f"child_{{k}}": k * j for k in range(10)}}
    expect(len(tree)).to_equal(30)
    flat = [v for children in tree.values() for v in children.values()]
    expect(len(flat)).to_equal(300)
    """),
]


def _generate_sync(n: int) -> str:
    lines = ["from tryke import expect, test\n\n"]
    for i in range(n):
        tmpl = TEMPLATES[i % len(TEMPLATES)]
        body = tmpl.format(seed=i * 100)
        indented = textwrap.indent(body, "    ")
        lines.append(f"@test\ndef test_sync_{i}():\n{indented}\n")
    return "\n".join(lines)


GENERATORS = {
    "sync": _generate_sync,
}


def main() -> None:
    # conftest to suppress the PytestCollectionWarning about the test decorator
    conftest = SUITES_DIR / "conftest.py"
    conftest.parent.mkdir(parents=True, exist_ok=True)
    conftest.write_text(
        textwrap.dedent("""\
        import pytest

        def pytest_configure(config):
            config.addinivalue_line(
                "filterwarnings",
                "ignore::pytest.PytestCollectionWarning",
            )
    """)
    )

    for scale in SCALES:
        suite_dir = SUITES_DIR / f"suite_{scale}"
        suite_dir.mkdir(parents=True, exist_ok=True)
        for kind, generator in GENERATORS.items():
            path = suite_dir / f"test_{kind}.py"
            path.write_text(generator(scale))
            sys.stdout.write(f"  wrote {path} ({scale} tests)\n")


if __name__ == "__main__":
    main()
