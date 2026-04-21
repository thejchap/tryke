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

## Using reporters with other modes

The `--reporter` flag works with [watch mode](watch-mode.md) too:

```bash
tryke watch --reporter dot
```
