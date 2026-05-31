"""Tests for the playground harness and the runner's no-executor path.

The playground module runs in Pyodide and writes user files to
``/home/pyodide/user``. The ``pyodide_root`` fixture points
``playground._PYODIDE_ROOT`` at a fresh temp dir so the tests can run in
the normal CPython worker, and restores the process-global state the
harness mutates (``_PYODIDE_ROOT``, ``sys.path``, ``sys.modules``) after
each test so nothing leaks into the rest of the suite.
"""

from __future__ import annotations

import json
import shutil
import sys
import tempfile
from pathlib import Path
from typing import TYPE_CHECKING

from tryke import Depends, describe, expect, fixture, test
from tryke import playground as _pg

if TYPE_CHECKING:
    from collections.abc import Generator


@fixture(per="test")
def pyodide_root() -> Generator[Path, None, None]:
    """Point the playground at a fresh temp dir, then restore global state.

    The harness mutates process-global state — ``_pg._PYODIDE_ROOT``,
    ``sys.path``, and the modules imported during ``run_tests``.
    Snapshotting and restoring around each test keeps that state from
    leaking into the rest of the suite, which shares the process.
    """
    saved_root = _pg._PYODIDE_ROOT  # noqa: SLF001
    saved_path = list(sys.path)
    saved_modules = set(sys.modules)

    root = Path(tempfile.mkdtemp(prefix="tryke-playground-"))
    _pg._PYODIDE_ROOT = root  # noqa: SLF001
    sys.path.insert(0, str(root / "user"))
    try:
        yield root
    finally:
        _pg._PYODIDE_ROOT = saved_root  # noqa: SLF001
        sys.path[:] = saved_path
        for name in set(sys.modules) - saved_modules:
            sys.modules.pop(name, None)
        shutil.rmtree(root, ignore_errors=True)


with describe("playground._write_files"):

    @test(name="creates parent directories for nested filenames")
    def test_creates_parent_dirs(root: Path = Depends(pyodide_root)) -> None:
        all_files = json.dumps(
            [{"name": "pkg/helpers.py", "source": "X = 1\n"}],
        )
        _pg._write_files("pkg/helpers.py", "X = 1\n", all_files)  # noqa: SLF001
        expect(
            (root / "user" / "pkg" / "helpers.py").read_text(),
            "nested file written with parent dirs",
        ).to_equal("X = 1\n")

    @test(name="rejects an unsafe active filename")
    def test_rejects_unsafe_active_filename(
        _root: Path = Depends(pyodide_root),
    ) -> None:
        all_files = json.dumps([{"name": "../escape.py", "source": ""}])
        expect(
            lambda: _pg._write_files("../escape.py", "", all_files),  # noqa: SLF001
            "unsafe active filename is rejected",
        ).to_raise(ValueError)

    @test(name="rejects an unsafe filename among the provided files")
    def test_rejects_unsafe_file_in_set(
        _root: Path = Depends(pyodide_root),
    ) -> None:
        all_files = json.dumps(
            [
                {"name": "test_main.py", "source": ""},
                {"name": "tryke.py", "source": "X = 1\n"},
            ],
        )
        expect(
            lambda: _pg._write_files("test_main.py", "", all_files),  # noqa: SLF001
            "unsafe file in the set is rejected",
        ).to_raise(ValueError)

    @test(name="single-file writes are reset by the next run")
    def test_single_file_mode_resets_user_root(
        root: Path = Depends(pyodide_root),
    ) -> None:
        _pg._write_files("leftover.py", "X = 1\n", None)  # noqa: SLF001
        expect(
            (root / "user" / "leftover.py").exists(),
            "single-file write lands in user root",
        ).to_be_truthy()

        current = json.dumps([{"name": "test_main.py", "source": ""}])
        _pg._write_files("test_main.py", "", current)  # noqa: SLF001
        expect(
            (root / "user" / "leftover.py").exists(),
            "previous single-file write is removed by sandbox reset",
        ).to_be_falsy()

    @test(name="removes files from the previous sandbox")
    def test_resets_user_root(root: Path = Depends(pyodide_root)) -> None:
        first = json.dumps(
            [
                {"name": "test_main.py", "source": ""},
                {"name": "helpers.py", "source": "X = 1\n"},
            ],
        )
        _pg._write_files("test_main.py", "", first)  # noqa: SLF001
        second = json.dumps([{"name": "test_main.py", "source": ""}])
        _pg._write_files("test_main.py", "", second)  # noqa: SLF001
        expect(
            (root / "user" / "helpers.py").exists(),
            "removed file is unlinked",
        ).to_be_falsy()


with describe("playground.run_tests"):

    @test(name="multi-file imports resolve via written files")
    def test_multi_file(_root: Path = Depends(pyodide_root)) -> None:
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

    @test(name="imported scope fixtures share one executor for the run")
    def test_imported_scope_fixture_lives_for_playground_run(
        _root: Path = Depends(pyodide_root),
    ) -> None:
        helpers = (
            "from tryke import fixture\n"
            "@fixture(per='scope')\n"
            "def db() -> list[int]:\n"
            "    return []\n"
        )
        test_source = (
            "from helpers import db\n"
            "from tryke import Depends, expect\n"
            "def first(db: list[int] = Depends(db)) -> None:\n"
            "    db.append(1)\n"
            "    expect(db, 'first receives shared db').to_equal([1])\n"
            "def second(db: list[int] = Depends(db)) -> None:\n"
            "    expect(db, 'second sees first mutation').to_equal([1])\n"
        )
        all_files = json.dumps(
            [
                {"name": "helpers.py", "source": helpers},
                {"name": "test_main.py", "source": test_source},
            ],
        )
        tests = json.dumps([{"name": "first"}, {"name": "second"}])
        out = _pg.run_tests("test_main.py", test_source, tests, all_files)
        results = json.loads(out)
        expect(
            [r["outcome"] for r in results],
            "scope fixture result is shared across tests",
        ).to_equal(["passed", "passed"])

    @test(name="doctest items are dispatched through doctest runner")
    def test_doctest_routing(_root: Path = Depends(pyodide_root)) -> None:
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
