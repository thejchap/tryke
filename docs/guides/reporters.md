# Reporters

Tryke supports multiple output formats via the `--reporter` flag.

```bash
tryke test --reporter <format>
```

## `text` (default)

The default reporter. Shows each test result with pass/fail status, assertion diagnostics on failure, and a summary at the end.

```bash
tryke test
tryke test --reporter text
```

## `dot`

Compact single-character output — one character per test. Useful for large suites where you only want to see failures:

- `.` pass
- `F` fail
- `E` error
- `s` skip
- `T` todo
- `x` xfail
- `X` xpassed (an `xfail` test that passed unexpectedly)

```bash
tryke test --reporter dot
```

## `json`

Machine-readable JSON output. Each test result is a JSON object, one per line (JSONL format). Useful for integrating with other tools or custom dashboards.

```bash
tryke test --reporter json
```

## `junit`

JUnit XML output for CI systems that consume JUnit reports (Jenkins, GitHub Actions, etc.):

```bash
tryke test --reporter junit > results.xml
```

## `llm`

A format optimized for consumption by large language models. Concise, structured output designed to fit in LLM context windows.

```bash
tryke test --reporter llm
```

## `next`

A cargo-nextest-style reporter. One line per completed test with a status badge, duration, and `file_stem :: test_name` identifier; a live status bar at the bottom of the terminal tracks progress through the run.

```bash
tryke test --reporter next
```

Sample output:

```text
     PASS  [  0.009s] test_one :: test_alpha
     FAIL  [  0.123s] test_one :: test_beta
  expected 1, got 2
     PASS  [  0.004s] test_two :: test_gamma
```

The status bar (`Running [00:00:02] [████████░░░░░░░░░░░░] 423/523 422 passed, 1 failed`) is drawn at the bottom of the terminal and only appears when both stdout and stderr are TTYs, so redirecting output to a file or piping into another command produces clean per-test lines with no escape codes.

## `sugar`

A pytest-sugar-style reporter. One line per test file showing inline check/cross marks for each test in the file, plus a count, percentage, and a small bar on the right. Failures are deferred to a recap at the end of the run, so the per-file output isn't interrupted.

```bash
tryke test --reporter sugar
```

Sample output:

```text
 tests/a.py ✓✗                                               2  66% ████████░░░░
 tests/b.py ✓                                                1 100% ████████████

Failures

✗ b (tests/a.py)
  boom
```

Like `next`, the live status bar is only drawn when both stdout and stderr are TTYs; redirecting either falls back to plain per-file lines with no escape codes.

## Using reporters with other modes

The `--reporter` flag works with [watch mode](watch-mode.md) too:

```bash
tryke test --watch --reporter dot
```
