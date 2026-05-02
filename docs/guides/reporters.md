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

Sample output (with `-v` to surface per-assertion lines):

<!-- REPORTER:text:START -->

```ansi
[1mtryke test[0m [2mv0.0.25[0m

sample.py:
  users
    get
      [31m![39m returns a stored user [31m[error][39m
    worker unavailable (spawn or hook replay failed)
    set
      [31m![39m stores a new user [31m[error][39m
    worker unavailable (spawn or hook replay failed)

 [2mTest Files[0m  [1m[31m1 ran[39m[0m [2m(1)[0m
      [2mTests[0m  [1m[31m2 error[39m[0m [2m(2)[0m
   [2mStart at[0m  10:02:24
   [2mDuration[0m  36.36ms [2m(discover 0.76ms, tests 35.60ms)[0m

 [1m[30;41m FAIL [0m[0m
```

<!-- REPORTER:text:END -->

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

Sample output:

<!-- REPORTER:dot:START -->

```ansi
[1mtryke test[0m [2mv0.0.25[0m

[31mE[39m[31mE[39m

 [2mTest Files[0m  [1m[31m1 ran[39m[0m [2m(1)[0m
      [2mTests[0m  [1m[31m2 error[39m[0m [2m(2)[0m
   [2mStart at[0m  10:02:24
   [2mDuration[0m  36.36ms [2m(discover 0.76ms, tests 35.60ms)[0m

 [1m[30;41m FAIL [0m[0m
```

<!-- REPORTER:dot:END -->

## `json`

Machine-readable JSON output. Each test result is a JSON object, one per line (JSONL format). Useful for integrating with other tools or custom dashboards.

```bash
tryke test --reporter json
```

Sample output:

<!-- REPORTER:json:START -->

```json
{"event":"run_start","tests":[{"name":"test_get","module_path":"sample","file_path":"sample.py","line_number":27,"display_name":"returns a stored user","expected_assertions":[{"subject":"users[\"alice\"]","matcher":"to_equal","negated":false,"args":["\"alice@example.com\""],"line":32,"label":"returns stored email"}],"groups":["users","get"]},{"name":"test_set","module_path":"sample","file_path":"sample.py","line_number":38,"display_name":"stores a new user","expected_assertions":[{"subject":"users[\"bob\"]","matcher":"to_equal","negated":false,"args":["\"bob@example.com\""],"line":41,"label":"stores email under user key"}],"groups":["users","set"]}]}
{"event":"test_complete","result":{"test":{"name":"test_get","module_path":"sample","file_path":"sample.py","line_number":27,"display_name":"returns a stored user","expected_assertions":[{"subject":"users[\"alice\"]","matcher":"to_equal","negated":false,"args":["\"alice@example.com\""],"line":32,"label":"returns stored email"}],"groups":["users","get"]},"outcome":{"status":"error","detail":{"message":"worker unavailable (spawn or hook replay failed)"}},"duration":{"secs":0,"nanos":0},"stdout":"","stderr":""}}
{"event":"test_complete","result":{"test":{"name":"test_set","module_path":"sample","file_path":"sample.py","line_number":38,"display_name":"stores a new user","expected_assertions":[{"subject":"users[\"bob\"]","matcher":"to_equal","negated":false,"args":["\"bob@example.com\""],"line":41,"label":"stores email under user key"}],"groups":["users","set"]},"outcome":{"status":"error","detail":{"message":"worker unavailable (spawn or hook replay failed)"}},"duration":{"secs":0,"nanos":0},"stdout":"","stderr":""}}
{"event":"run_complete","summary":{"passed":0,"failed":0,"skipped":0,"errors":2,"xfailed":0,"todo":0,"duration":{"secs":0,"nanos":0},"discovery_duration":{"secs":0,"nanos":760000},"test_duration":{"secs":0,"nanos":35600000},"file_count":1,"start_time":"10:02:24"}}
```

<!-- REPORTER:json:END -->

## `junit`

JUnit XML output for CI systems that consume JUnit reports (Jenkins, GitHub Actions, etc.):

```bash
tryke test --reporter junit > results.xml
```

Sample output:

<!-- REPORTER:junit:START -->

```xml
<?xml version="1.0" encoding="UTF-8"?>
<testsuite name="tryke" tests="2" failures="0" errors="2" skipped="0" time="0.036">
  <testcase name="returns a stored user" classname="sample.users.get" time="0.000">
    <error message="worker unavailable (spawn or hook replay failed)"/>
  </testcase>
  <testcase name="stores a new user" classname="sample.users.set" time="0.000">
    <error message="worker unavailable (spawn or hook replay failed)"/>
  </testcase>
</testsuite>
```

<!-- REPORTER:junit:END -->

## `llm`

A format optimized for consumption by large language models. Concise, structured output designed to fit in LLM context windows.

```bash
tryke test --reporter llm
```

Sample output:

<!-- REPORTER:llm:START -->

```text
ERROR returns a stored user (sample.py:27)
  worker unavailable (spawn or hook replay failed)
ERROR stores a new user (sample.py:38)
  worker unavailable (spawn or hook replay failed)
2 error [36.36ms]
```

<!-- REPORTER:llm:END -->

## `next`

A cargo-nextest-style reporter. One line per completed test with a status badge, duration, and `file_stem :: test_name` identifier; a live status bar at the bottom of the terminal tracks progress through the run.

```bash
tryke test --reporter next
```

Sample output:

<!-- REPORTER:next:START -->

```ansi
[1mtryke test[0m [2mv0.0.25[0m

     [1m[31mERROR[39m[0m [[2m  0.000s[0m] [1m[36msample[39m[0m [2m>[0m [36musers[39m [2m>[0m [36mget[39m [2m::[0m returns a stored user
    worker unavailable (spawn or hook replay failed)
     [1m[31mERROR[39m[0m [[2m  0.000s[0m] [1m[36msample[39m[0m [2m>[0m [36musers[39m [2m>[0m [36mset[39m [2m::[0m stores a new user
    worker unavailable (spawn or hook replay failed)

 [2mTest Files[0m  [1m[31m1 ran[39m[0m [2m(1)[0m
      [2mTests[0m  [1m[31m2 error[39m[0m [2m(2)[0m
   [2mStart at[0m  10:02:24
   [2mDuration[0m  36.36ms [2m(discover 0.76ms, tests 35.60ms)[0m

 [1m[30;41m FAIL [0m[0m
```

<!-- REPORTER:next:END -->

The status bar (`Running [00:00:02] [████████░░░░░░░░░░░░] 423/523 422 passed, 1 failed`) is drawn at the bottom of the terminal and only appears when both stdout and stderr are TTYs, so redirecting output to a file or piping into another command produces clean per-test lines with no escape codes.

## `sugar`

A pytest-sugar-style reporter. One line per test file showing inline check/cross marks for each test in the file, plus a count, percentage, and a small bar on the right. Failures are deferred to a recap at the end of the run, so the per-file output isn't interrupted.

```bash
tryke test --reporter sugar
```

Sample output:

<!-- REPORTER:sugar:START -->

```ansi
[1mtryke test[0m [2mv0.0.25[0m

 [1msample.py[0m [1m[31mE[39m[0m[1m[31mE[39m[0m                                                [1m2[0m [1m100%[0m [31m████████████[39m

[4m[1m[31mFailures[39m[0m[0m

[1m[31m✗[39m[0m returns a stored user [2m(sample.py)[0m
    worker unavailable (spawn or hook replay failed)

[1m[31m✗[39m[0m stores a new user [2m(sample.py)[0m
    worker unavailable (spawn or hook replay failed)

 [2mTest Files[0m  [1m[31m1 ran[39m[0m [2m(1)[0m
      [2mTests[0m  [1m[31m2 error[39m[0m [2m(2)[0m
   [2mStart at[0m  10:02:24
   [2mDuration[0m  36.36ms [2m(discover 0.76ms, tests 35.60ms)[0m

 [1m[30;41m FAIL [0m[0m
```

<!-- REPORTER:sugar:END -->

Like `next`, the live status bar is only drawn when both stdout and stderr are TTYs; redirecting either falls back to plain per-file lines with no escape codes.

## Using reporters with other modes

The `--reporter` flag works with [watch mode](watch-mode.md) too:

```bash
tryke test --watch --reporter dot
```
