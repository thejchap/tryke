"""Lightweight guard for tryke in-source testing.

Importing this module is cheap: it reads one environment variable and does
nothing else. Use it in production modules to guard test code that should
not run or load its dependencies outside of a tryke test run.

Example:
    ```python
    from tryke_guard import __TRYKE_TESTING__

    def add(a, b):
        return a + b

    if __TRYKE_TESTING__:
        from tryke import test, expect

        @test
        def adds():
            expect(add(1, 2)).to_equal(3)
    ```

In production ``__TRYKE_TESTING__`` is ``False``, the ``if`` block is dead
code, and the heavy tryke API never loads. Under ``tryke test`` the worker
flips the module attribute to ``True`` at startup (``tryke_guard.__TRYKE_TESTING__
= True``), the block runs, and the test is discovered and executed like any
other.

The worker deliberately mutates the module attribute rather than setting an
env var. Env vars propagate to subprocesses; module attributes do not.
Children spawned by user tests (``subprocess.run`` / ``multiprocessing.Process``
with ``spawn``) therefore start with a fresh ``tryke_guard`` import that sees
``__TRYKE_TESTING__ = False`` — production mode by default. This matches the
"strip test code in production" ethos: if a test launches the user's app as a
subprocess to black-box-test it, the subprocess behaves like prod.

To explicitly opt a subprocess into test mode, pass
``env={**os.environ, "TRYKE_TESTING": "1"}`` when launching it; the env-var
path below recognises that as another way to flip the flag on at import time.
"""

import os

__TRYKE_TESTING__: bool = os.environ.get("TRYKE_TESTING") == "1"

__all__ = ["__TRYKE_TESTING__"]
