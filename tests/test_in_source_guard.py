"""Dogfood tests for `tryke_guard.__TRYKE_TESTING__` in-source testing.

Runs under tryke's own test suite via ``cargo run test``. Verifies:

- `tryke_guard.__TRYKE_TESTING__` is `True` inside the worker.
- Tests defined inside `if __TRYKE_TESTING__:` are discovered and runnable.
- A subprocess spawned by a test sees `__TRYKE_TESTING__ == False` by default
  (production-mode children).
- An explicitly-opted-in subprocess (via `TRYKE_TESTING=1` env var) sees `True`.
"""

from __future__ import annotations

import os
import subprocess
import sys

from tryke import describe, expect, test
from tryke_guard import __TRYKE_TESTING__

with describe("tryke_guard"):

    @test
    def worker_sees_testing_true() -> None:
        expect(__TRYKE_TESTING__, "testing flag is true in worker").to_be_truthy()

    if __TRYKE_TESTING__:
        # This block only executes when running under tryke. Its sole purpose
        # is to prove that discovery + runtime wiring both work: the test
        # below is defined inside the guard, so if it runs at all the
        # round-trip is healthy.
        @test
        def test_inside_guard_runs() -> None:
            expect(1 + 1, "guarded test body runs").to_equal(2)

    @test
    def subprocess_defaults_to_production_mode() -> None:
        out = subprocess.check_output(
            [
                sys.executable,
                "-c",
                "from tryke_guard import __TRYKE_TESTING__; print(__TRYKE_TESTING__)",
            ],
            text=True,
        ).strip()
        expect(out, "subprocess sees testing flag false by default").to_equal("False")

    @test
    def subprocess_opts_in_with_env_var() -> None:
        out = subprocess.check_output(
            [
                sys.executable,
                "-c",
                "from tryke_guard import __TRYKE_TESTING__; print(__TRYKE_TESTING__)",
            ],
            text=True,
            env={**os.environ, "TRYKE_TESTING": "1"},
        ).strip()
        expect(out, "subprocess with TRYKE_TESTING=1 sees true").to_equal("True")
