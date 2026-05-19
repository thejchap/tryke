"""Tests for the playground harness and the runner's no-executor path.

The playground module runs in Pyodide and writes user files to
``/home/pyodide`` (the Pyodide home dir). These tests monkeypatch
``playground._PYODIDE_ROOT`` to a temp dir so they can run in the
normal CPython worker.
"""

from __future__ import annotations

import asyncio
import json
import sys
import tempfile
from pathlib import Path

from tryke import Depends, describe, expect, fixture, test
from tryke import playground as _pg
from tryke.runner import _run_without_executor


def _fresh_pyodide_root() -> Path:
    """Allocate a clean temp dir, point playground at it, and add it to sys.path."""
    root = Path(tempfile.mkdtemp(prefix="tryke-playground-"))
    _pg._PYODIDE_ROOT = root  # noqa: SLF001
    _pg._WRITTEN_FILES.clear()  # noqa: SLF001
    if str(root) not in sys.path:
        sys.path.insert(0, str(root))
    return root


with describe("_run_without_executor"):

    @test(name="case kwarg colliding with Depends fixture raises TypeError")
    def test_collision_raises() -> None:
        @fixture
        def db() -> str:
            return "real"

        def my_test(db: str = Depends(db)) -> None:
            pass

        my_test.__name__ = "my_test"
        # Collision: case provides `db`, but a fixture-injected `db` already exists.
        raised: TypeError | None = None
        try:
            _run_without_executor(my_test, (), {"db": "fake"})
        except TypeError as exc:
            raised = exc
        expect(raised is not None, "TypeError was raised on collision").to_be_truthy()
        expect(
            str(raised) if raised else "",
            "collision message names parameter",
        ).to_contain("db")

    @test(name="async test runs on the resolver's shared loop")
    def test_async_uses_shared_loop() -> None:
        captured: dict[str, object] = {}

        async def my_test() -> None:
            captured["loop"] = asyncio.get_running_loop()

        _run_without_executor(my_test, (), None)
        loop = captured["loop"]
        expect(loop is not None, "async test body ran").to_be_truthy()
        # The shared loop is closed after _run_without_executor finishes,
        # which is the contract: it lives only for the duration of a single
        # playground test run and is then torn down with the resolver.
        expect(
            isinstance(loop, asyncio.AbstractEventLoop),
            "shared loop is a real event loop",
        ).to_be_truthy()

    @test(name="async fixture and async test share one loop")
    def test_async_fixture_and_test_share_loop() -> None:
        loops: dict[str, asyncio.AbstractEventLoop] = {}

        @fixture
        async def setup() -> str:
            loops["fixture"] = asyncio.get_running_loop()
            return "ready"

        async def my_test(setup: str = Depends(setup)) -> None:
            loops["test"] = asyncio.get_running_loop()
            expect(setup, "fixture value is injected").to_equal("ready")

        _run_without_executor(my_test, (), None)
        expect(
            loops["fixture"] is loops["test"],
            "fixture and test ran on the same event loop",
        ).to_be_truthy()


with describe("playground._write_files"):

    @test(name="creates parent directories for nested filenames")
    def test_creates_parent_dirs() -> None:
        root = _fresh_pyodide_root()
        all_files = json.dumps(
            [{"name": "pkg/helpers.py", "source": "X = 1\n"}],
        )
        _pg._write_files("pkg/helpers.py", "X = 1\n", all_files)  # noqa: SLF001
        expect(
            (root / "pkg" / "helpers.py").read_text(),
            "nested file written with parent dirs",
        ).to_equal("X = 1\n")

    @test(name="purges files removed since the previous run")
    def test_purges_removed_files() -> None:
        root = _fresh_pyodide_root()
        first = json.dumps(
            [
                {"name": "test_main.py", "source": ""},
                {"name": "helpers.py", "source": "X = 1\n"},
            ],
        )
        _pg._write_files("test_main.py", "", first)  # noqa: SLF001
        # Pre-load helpers so we can verify the purge.
        sys.modules["helpers"] = sys.modules.get("helpers") or type(sys)("helpers")
        second = json.dumps([{"name": "test_main.py", "source": ""}])
        _pg._write_files("test_main.py", "", second)  # noqa: SLF001
        expect(
            (root / "helpers.py").exists(),
            "removed file is unlinked",
        ).to_be_falsy()
        expect(
            "helpers" in sys.modules,
            "removed module is dropped from sys.modules",
        ).to_be_falsy()

    @test(name="package __init__.py purges the package name, not pkg.__init__")
    def test_init_purge() -> None:
        _fresh_pyodide_root()
        all_files = json.dumps(
            [{"name": "pkg/__init__.py", "source": "VALUE = 1\n"}],
        )
        # Seed sys.modules with what import would have cached.
        sys.modules["pkg"] = type(sys)("pkg")
        _pg._write_files("pkg/__init__.py", "VALUE = 1\n", all_files)  # noqa: SLF001
        expect("pkg" in sys.modules, "package alias purged").to_be_falsy()


with describe("playground.run_tests"):

    @test(name="multi-file imports resolve via written files")
    def test_multi_file() -> None:
        _fresh_pyodide_root()
        helpers = "VALUE = 42\n"
        test_source = (
            "from helpers import VALUE\n"
            "def my_test() -> None:\n"
            "    assert VALUE == 42\n"
        )
        all_files = json.dumps(
            [
                {"name": "helpers.py", "source": helpers},
                {"name": "test_main.py", "source": test_source},
            ],
        )
        tests = json.dumps([{"name": "my_test"}])
        out = _pg.run_tests("test_main.py", test_source, tests, all_files)
        results = json.loads(out)
        expect(len(results), "one test result").to_equal(1)
        expect(results[0]["outcome"], "import-from-helpers test passes").to_equal(
            "passed"
        )

    @test(name="doctest items are dispatched through doctest runner")
    def test_doctest_routing() -> None:
        _fresh_pyodide_root()
        source = (
            "def add(a: int, b: int) -> int:\n"
            '    """Add two numbers.\n'
            "\n"
            "    >>> add(1, 2)\n"
            "    3\n"
            '    """\n'
            "    return a + b\n"
        )
        tests = json.dumps([{"name": "add", "doctest_object": "add"}])
        out = _pg.run_tests("mod_doctest.py", source, tests, None)
        results = json.loads(out)
        expect(len(results), "one doctest result").to_equal(1)
        expect(results[0]["outcome"], "doctest passes").to_equal("passed")
