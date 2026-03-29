from __future__ import annotations

import asyncio
import io
import json
import sys
import traceback
import types
import unittest

from tryke import describe, expect, test
from tryke.expect import ExpectationError, SoftFailure
from tryke.worker import (
    _TRYKE_PKG,
    Worker,
    _extract_soft_failures,
    _is_user_frame,
    _make_assertion_wire,
)

# -- Helpers ------------------------------------------------------------------


def _rpc(method: str, id_: int = 1, **params: object) -> dict:
    """Build a JSON-RPC request dict."""
    req: dict[str, object] = {
        "jsonrpc": "2.0",
        "id": id_,
        "method": method,
    }
    if params:
        req["params"] = params
    return req


def _send(request: dict) -> dict:
    """Send one request through a fresh Worker and return the response."""
    input_buf = io.StringIO(json.dumps(request) + "\n")
    output_buf = io.StringIO()
    Worker(input_buf, output_buf).run()
    return json.loads(output_buf.getvalue().strip())


def _run_test_fn(
    fn: object,
    *,
    xfail: str | None = None,
) -> dict:
    """Execute *fn* via the worker run_test path and return the result."""
    mod = types.ModuleType("_tw")
    mod.test_fn = fn
    params: dict[str, object] = {
        "module": "_tw",
        "function": "test_fn",
    }
    if xfail is not None:
        params["xfail"] = xfail
    req: dict[str, object] = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "run_test",
        "params": params,
    }
    input_buf = io.StringIO(json.dumps(req) + "\n")
    output_buf = io.StringIO()
    worker = Worker(input_buf, output_buf)
    worker._modules["_tw"] = mod  # noqa: SLF001
    worker.run()
    resp = json.loads(output_buf.getvalue().strip())
    return resp["result"]


# -- JSON-RPC envelope -------------------------------------------------------


with describe("json-rpc envelope"):

    @test(name="ping returns pong")
    def test_ping() -> None:
        resp = _send(_rpc("ping"))
        expect(resp["result"]).to_equal("pong")
        expect(resp["id"]).to_equal(1)

    @test(name="malformed JSON returns parse error")
    def test_malformed_json() -> None:
        input_buf = io.StringIO("not json\n")
        output_buf = io.StringIO()
        Worker(input_buf, output_buf).run()
        resp = json.loads(output_buf.getvalue().strip())
        expect(resp["error"]["code"]).to_equal(-32700)

    @test(name="empty lines are skipped")
    def test_empty_lines() -> None:
        payload = json.dumps(_rpc("ping"))
        input_buf = io.StringIO(f"\n\n{payload}\n\n")
        output_buf = io.StringIO()
        Worker(input_buf, output_buf).run()
        lines = [ln for ln in output_buf.getvalue().split("\n") if ln.strip()]
        expect(lines).to_have_length(1)

    @test(name="unknown method returns internal error")
    def test_unknown_method() -> None:
        resp = _send(_rpc("bogus"))
        expect(resp["error"]["code"]).to_equal(-32603)
        expect(resp["error"]["message"]).to_contain(
            "unknown method",
        )

    @test(name="missing required param returns -32602")
    def test_missing_param() -> None:
        resp = _send(_rpc("run_test"))
        expect(resp["error"]["code"]).to_equal(-32602)
        expect(resp["error"]["message"]).to_contain(
            "requires parameter",
        )

    @test(name="non-string param returns -32602")
    def test_bad_param_type() -> None:
        req: dict[str, object] = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "run_test",
            "params": {"module": 123, "function": "f"},
        }
        resp = _send(req)
        expect(resp["error"]["code"]).to_equal(-32602)
        expect(resp["error"]["message"]).to_contain(
            "must be a string",
        )


# -- run_test outcomes --------------------------------------------------------


with describe("run_test outcomes"):

    @test(name="passing test")
    def test_passing() -> None:
        def fn() -> None:
            pass

        result = _run_test_fn(fn)
        expect(result["outcome"]).to_equal("passed")
        expect(result["duration_ms"]).to_be_greater_than_or_equal(
            0,
        )

    @test(name="failing with ExpectationError")
    def test_expectation_error() -> None:
        def fn() -> None:
            msg = "bad"
            raise ExpectationError(
                msg,
                expected="1",
                received="2",
            )

        result = _run_test_fn(fn)
        expect(result["outcome"]).to_equal("failed")
        expect(result["message"]).to_contain("bad")
        expect(len(result["assertions"])).to_be_greater_than(0)

    @test(name="soft assertion failures are collected")
    def test_soft_failures() -> None:
        def fn() -> None:
            # These run inside the worker's soft-assertion context,
            # so they collect failures instead of raising.
            expect(1).to_equal(2)
            expect(3).to_equal(4)

        result = _run_test_fn(fn)
        expect(result["outcome"]).to_equal("failed")
        expect(result["assertions"]).to_have_length(2)

    @test(name="general exception")
    def test_general_exception() -> None:
        def fn() -> None:
            msg = "boom"
            raise RuntimeError(msg)

        result = _run_test_fn(fn)
        expect(result["outcome"]).to_equal("failed")
        expect(result["message"]).to_contain("RuntimeError")
        expect(result["message"]).to_contain("boom")

    @test(name="AssertionError (non-ExpectationError)")
    def test_plain_assertion_error() -> None:
        def fn() -> None:
            msg = "plain assert"
            raise AssertionError(msg)

        result = _run_test_fn(fn)
        expect(result["outcome"]).to_equal("failed")
        expect(result["message"]).to_contain("plain assert")

    @test(name="skipped marker")
    def test_skip_marker() -> None:
        def fn() -> None:
            pass

        fn.__tryke_skip__ = "not ready"  # type: ignore[attr-defined]
        result = _run_test_fn(fn)
        expect(result["outcome"]).to_equal("skipped")
        expect(result["reason"]).to_equal("not ready")

    @test(name="todo marker")
    def test_todo_marker() -> None:
        def fn() -> None:
            pass

        fn.__tryke_todo__ = "implement later"  # type: ignore[attr-defined]
        result = _run_test_fn(fn)
        expect(result["outcome"]).to_equal("todo")
        expect(result["description"]).to_equal("implement later")

    @test(name="xfail that fails returns xfailed")
    def test_xfail_fails() -> None:
        def fn() -> None:
            msg = "expected"
            raise ValueError(msg)

        result = _run_test_fn(fn, xfail="known bug")
        expect(result["outcome"]).to_equal("xfailed")
        expect(result["reason"]).to_equal("known bug")

    @test(name="xfail that passes returns xpassed")
    def test_xfail_passes() -> None:
        def fn() -> None:
            pass

        result = _run_test_fn(fn, xfail="should fail")
        expect(result["outcome"]).to_equal("xpassed")

    @test(name="xfail via marker attribute")
    def test_xfail_marker() -> None:
        def fn() -> None:
            msg = "fail"
            raise ValueError(msg)

        fn.__tryke_xfail__ = "marker reason"  # type: ignore[attr-defined]
        result = _run_test_fn(fn)
        expect(result["outcome"]).to_equal("xfailed")
        expect(result["reason"]).to_equal("marker reason")

    @test(name="unittest.SkipTest returns skipped")
    def test_unittest_skip() -> None:
        def fn() -> None:
            msg = "conditional"
            raise unittest.SkipTest(msg)

        result = _run_test_fn(fn)
        expect(result["outcome"]).to_equal("skipped")
        expect(result["reason"]).to_equal("conditional")

    @test(name="async test executes correctly")
    def test_async_test() -> None:
        async def fn() -> None:
            await asyncio.sleep(0)

        result = _run_test_fn(fn)
        expect(result["outcome"]).to_equal("passed")

    @test(name="stdout and stderr are captured")
    def test_output_capture() -> None:
        def fn() -> None:
            print("hello stdout")  # noqa: T201
            print("hello stderr", file=sys.stderr)  # noqa: T201

        result = _run_test_fn(fn)
        expect(result["stdout"]).to_contain("hello stdout")
        expect(result["stderr"]).to_contain("hello stderr")

    @test(name="import error returns failed with traceback")
    def test_import_error() -> None:
        resp = _send(
            _rpc(
                "run_test",
                module="nonexistent_module_xyz_12345",
                function="test_fn",
            ),
        )
        # Should be a successful RPC response (result, not error)
        expect("result" in resp).to_be_truthy()
        expect("error" in resp).to_be_falsy()
        result = resp["result"]
        expect(result["outcome"]).to_equal("failed")
        expect(result["message"]).to_contain("ModuleNotFoundError")
        expect(result["traceback"]).to_be_truthy()
        expect(result["traceback"]).to_contain("ModuleNotFoundError")

    @test(name="attribute error returns failed with traceback")
    def test_attribute_error() -> None:
        # Module exists but function does not
        mod = types.ModuleType("_tw_attr")
        req = _rpc(
            "run_test",
            module="_tw_attr",
            function="no_such_function",
        )
        input_buf = io.StringIO(json.dumps(req) + "\n")
        output_buf = io.StringIO()
        worker = Worker(input_buf, output_buf)
        worker._modules["_tw_attr"] = mod  # noqa: SLF001
        worker.run()
        resp = json.loads(output_buf.getvalue().strip())
        expect("result" in resp).to_be_truthy()
        result = resp["result"]
        expect(result["outcome"]).to_equal("failed")
        expect(result["message"]).to_contain("AttributeError")
        expect(result["traceback"]).to_be_truthy()


# -- run_doctest --------------------------------------------------------------


def _add(a: int, b: int) -> int:
    """Add two numbers.

    >>> _add(1, 2)
    3
    """
    return a + b


def _run_doctest_fn(
    fn: object,
    *,
    fn_name: str = "target",
    mod_name: str = "_tw_dt",
) -> dict:
    """Execute a doctest through the worker and return the result."""
    mod = types.ModuleType(mod_name)
    setattr(mod, fn_name, fn)
    req: dict[str, object] = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "run_doctest",
        "params": {"module": mod_name, "object_path": fn_name},
    }
    input_buf = io.StringIO(json.dumps(req) + "\n")
    output_buf = io.StringIO()
    worker = Worker(input_buf, output_buf)
    worker._modules[mod_name] = mod  # noqa: SLF001
    worker.run()
    resp = json.loads(output_buf.getvalue().strip())
    return resp["result"]


with describe("run_doctest"):

    @test(name="passing doctest")
    def test_doctest_pass() -> None:
        result = _run_doctest_fn(_add)
        expect(result["outcome"]).to_equal("passed")

    @test(name="failing doctest")
    def test_doctest_fail() -> None:
        # Build the function with its own globals so the doctest
        # can resolve the function name without polluting module scope.
        def _impl() -> int:
            return 1

        globs: dict[str, object] = {}
        bad = types.FunctionType(
            _impl.__code__,
            globs,
            "bad",
        )
        bad.__doc__ = ">>> bad()\n99\n"
        globs["bad"] = bad
        result = _run_doctest_fn(bad, fn_name="bad")
        expect(result["outcome"]).to_equal("failed")

    @test(name="failing doctest does not leak summarize output to stdout")
    def test_doctest_fail_no_stdout_leak() -> None:
        # When the worker's output stream IS sys.stdout (the production
        # setup), DocTestRunner.summarize() must not write to it —
        # otherwise the extra text corrupts the JSON-RPC channel.
        def _impl() -> int:
            return 1

        globs: dict[str, object] = {}
        bad = types.FunctionType(
            _impl.__code__,
            globs,
            "bad",
        )
        bad.__doc__ = ">>> bad()\n99\n"
        globs["bad"] = bad

        mod_name = "_tw_dt_leak"
        mod = types.ModuleType(mod_name)
        mod.bad = bad

        req: dict[str, object] = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "run_doctest",
            "params": {"module": mod_name, "object_path": "bad"},
        }
        input_buf = io.StringIO(json.dumps(req) + "\n")
        output_buf = io.StringIO()

        # Simulate production: worker writes to the same stream as
        # sys.stdout so summarize() pollution would be visible.
        old_stdout = sys.stdout
        sys.stdout = output_buf
        try:
            worker = Worker(input_buf, output_buf)
            worker._modules[mod_name] = mod  # noqa: SLF001
            worker.run()
        finally:
            sys.stdout = old_stdout

        raw = output_buf.getvalue()
        lines = [line for line in raw.splitlines() if line.strip()]
        expect(len(lines)).to_equal(1)
        resp = json.loads(lines[0])
        expect(resp["result"]["outcome"]).to_equal("failed")

    @test(name="import error in doctest returns failed with traceback")
    def test_doctest_import_error() -> None:
        resp = _send(
            _rpc(
                "run_doctest",
                module="nonexistent_module_xyz_12345",
                object_path="Foo",
            ),
        )
        expect("result" in resp).to_be_truthy()
        expect("error" in resp).to_be_falsy()
        result = resp["result"]
        expect(result["outcome"]).to_equal("failed")
        expect(result["message"]).to_contain("ModuleNotFoundError")
        expect(result["traceback"]).to_be_truthy()

    @test(name="attribute error in doctest returns failed with traceback")
    def test_doctest_attribute_error() -> None:
        mod = types.ModuleType("_tw_dt_attr")
        req = _rpc(
            "run_doctest",
            module="_tw_dt_attr",
            object_path="no_such_attr",
        )
        input_buf = io.StringIO(json.dumps(req) + "\n")
        output_buf = io.StringIO()
        worker = Worker(input_buf, output_buf)
        worker._modules["_tw_dt_attr"] = mod  # noqa: SLF001
        worker.run()
        resp = json.loads(output_buf.getvalue().strip())
        expect("result" in resp).to_be_truthy()
        result = resp["result"]
        expect(result["outcome"]).to_equal("failed")
        expect(result["message"]).to_contain("AttributeError")
        expect(result["traceback"]).to_be_truthy()


# -- Assertion helpers --------------------------------------------------------


with describe("assertion helpers"):

    @test(name="make_assertion_wire with frame")
    def test_wire_with_frame() -> None:
        frame = traceback.FrameSummary(
            "test.py",
            42,
            "test_fn",
            lookup_line=False,
            line="expect(x).to_equal(y)",
        )
        wire = _make_assertion_wire(
            expression="expect(x).to_equal(y)",
            expected="1",
            received="2",
            frame=frame,
        )
        expect(wire["expression"]).to_equal(
            "expect(x).to_equal(y)",
        )
        expect(wire["expected"]).to_equal("1")
        expect(wire["received"]).to_equal("2")
        expect(wire["line"]).to_equal(42)
        expect(wire["file"]).to_equal("test.py")

    @test(name="make_assertion_wire without frame omits location")
    def test_wire_no_frame() -> None:
        wire = _make_assertion_wire(
            expression="",
            expected="a",
            received="b",
        )
        expect(wire["expected"]).to_equal("a")
        expect("line" in wire).to_be_falsy()
        expect("file" in wire).to_be_falsy()

    @test(name="extract_soft_failures with frame")
    def test_extract_with_frame() -> None:
        frame = traceback.FrameSummary(
            "t.py",
            10,
            "fn",
            lookup_line=False,
            line="expect(1).to_equal(2)",
        )
        err = ExpectationError(
            "e",
            expected="1",
            received="2",
        )
        result = _extract_soft_failures(
            [SoftFailure(err, frame)],
        )
        expect(result).to_have_length(1)
        expect(result[0]["expected"]).to_equal("1")
        expect(result[0]["line"]).to_equal(10)
        expect(result[0]["expression"]).to_equal(
            "expect(1).to_equal(2)",
        )

    @test(name="extract_soft_failures without frame")
    def test_extract_no_frame() -> None:
        err = ExpectationError(
            "e",
            expected="a",
            received="b",
        )
        result = _extract_soft_failures(
            [SoftFailure(err, None)],
        )
        expect(result).to_have_length(1)
        expect(result[0]["expression"]).to_equal("")
        expect("line" in result[0]).to_be_falsy()
        expect("file" in result[0]).to_be_falsy()

    @test(name="is_user_frame identifies user frames")
    def test_user_frame() -> None:
        frame = traceback.FrameSummary(
            "/home/user/test.py",
            1,
            "fn",
            lookup_line=False,
        )
        expect(_is_user_frame(frame)).to_be_truthy()

    @test(name="is_user_frame excludes tryke frames")
    def test_tryke_frame() -> None:
        frame = traceback.FrameSummary(
            _TRYKE_PKG + "/expect.py",
            1,
            "fn",
            lookup_line=False,
        )
        expect(_is_user_frame(frame)).to_be_falsy()
