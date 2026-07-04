## 0.0.30

Released on 2026-07-04.

### Breaking Changes

- Replace server TCP transport with stdio JSON-RPC (#148)

### Documentation

- Mention uvx quickstart and playground in README and getting started (#127)

### Features

- Add configurable discovery cache directory (#128)
- Add clean cache command (#129)
- Skip rediscovery for unchanged AST (#130)
- Add interactive browser playground (#126)

### Contributors

- @thejchap
- @RaulBSanchez
- @dependabot[bot]
- @marvin8
- @sarvesh1327

## unreleased

### Documentation

- Mention uvx quickstart and playground in README and getting started (#127)

### Features

- Add configurable discovery cache directory (#128)
- Add clean cache command (#129)
- Add `--workers` to `tryke server`
- Discover Python environments from `VIRTUAL_ENV`, Conda, and the project
  `.venv`

### Bug Fixes

- Prevent watch and server runs from hanging while restarting worker processes
- Prevent concurrent worker spawn timeouts with relative Python paths on macOS
- Preserve virtual-environment interpreter symlinks during path resolution

### Contributors

- @sarvesh1327
- @thejchap

## 0.0.29

Released on 2026-05-18.

### Bug Fixes

- Skip teardown drain when event loop is closed (#109)
- Surface worker spawn and hook replay errors in test results (#107)
- Honor --collect-only in next, sugar, dot, and junit reporters (#115)
- Close stale worker race with in-band did_change RPC (#118)
- Fix multiline CLI diagnostics (#124)

### Documentation

- Add highlights to README and align getting started with README (#121)

### Features

- Format durations over a minute as M:SS.SS in reporter summaries (#116)
- Remove generated examples and default to watch mode (#120)
- Highlight received assertion values (#123)
- Add watch shortcut to clear results (#125)

### Contributors

- @thejchap

## 0.0.28

Released on 2026-05-08.

### Features

- Add --now flag to run tests immediately on watch startup (#102)

### Contributors

- @thejchap
- @dependabot[bot]

## 0.0.27

Released on 2026-05-03.

### Features

- Unify Rust and Python worker logging via TRYKE_LOG (#94)

### Contributors

- @thejchap

## 0.0.26

Released on 2026-05-02.

### Breaking Changes

- Fold tryke watch into tryke test --watch (#73)

### Bug Fixes

- Remove interpreter version check from tryke_runner (#92)

### Documentation

- Improve CLI documentation generation with detailed help text (#79)
- Update documentation examples with labeled expectations (#87)

### Features

- Support labeled test cases with @test("label").cases(...) (#90)

### Contributors

- @thejchap

## 0.0.25

Released on 2026-04-28.

### Features

- Add next and sugar reporters (#71)
- Add --no-progress flag to disable terminal progress bars (#72)

### Contributors

- @thejchap

## 0.0.24

Released on 2026-04-26.

### Bug Fixes

- Drain worker stderr to prevent RPC hangs on chatty suites (#66)
- Replace importlib.reload with worker subprocess restart (#68)

### Documentation

- Refresh stale tryke test banner version to 0.0.23 (#65)

### Features

- Restrict discovery walk and skip import graph when paths are passed (#64)
- Accept Annotated[T, Depends(...)] alongside default-form dependencies (#67)

### Contributors

- @thejchap

## 0.0.23

Released on 2026-04-25.

### Contributors

- @thejchap

## 0.0.22

Released on 2026-04-25.

### Bug Fixes

- Replay hooks on respawn and stop unsafe test retries (#58)

### Features

- Add --all flag to rerun the full test set on every change (#59)
- Track PEP 810 lazy imports in dependency graph (#60)
- Resolve absolute imports across multiple source roots (#61)
- Add test labels (#62)

### Contributors

- @thejchap
- @claude

## 0.0.21

Released on 2026-04-23.

### Contributors

- @thejchap

## 0.0.20

Released on 2026-04-23.

### Contributors

- @thejchap

## 0.0.19

Released on 2026-04-23.

### Bug Fixes

- Share event loop across async fixtures and tests, and accept patch versions (#51)
- Tag server runs with run_id and serialize concurrent runs (#54)

### Documentation

- Add comprehensive pytest-to-Tryke migration guide (#49)
- Correct dot-reporter glyphs and add to_be_instance_of matcher (#53)
- Add session continuity and commit cadence to LLM migration prompt (#55)
- Drop stale _all/before_all hook terminology and fix --lf migration row (#56)

### Features

- Add to_be_instance_of matcher for type checking (#50)
- Add fixtures, parametrization, Python 3.15 support, and docs (#52)

### Contributors

- @thejchap
- @claude

## 0.0.18

Released on 2026-04-19.

### Bug Fixes

- Ship python/tryke_guard.py in the built wheel (#48)

### Contributors

- @thejchap
- @claude

## 0.0.17

Released on 2026-04-18.

### Features

- Add @test.cases parametrization primitive (#46)
- Add in-source testing via tryke_guard.__TRYKE_TESTING__ (#47)

### Contributors

- @thejchap

## 0.0.15

Released on 2026-04-15.

### Contributors

- @thejchap

## 0.0.14

Released on 2026-04-14.

### Features

- Add @fixture and Depends() for setup, teardown, and DI (#39)

### Contributors

- @thejchap

## 0.0.13

Released on 2026-03-29.

### Contributors

- @thejchap
- @dependabot[bot]

## 0.0.12

Released on 2026-03-26.

### Contributors

- @thejchap

## 0.0.11

Released on 2026-03-21.

### Contributors

- @thejchap

## 0.0.10

Released on 2026-03-17.

### Contributors

- @thejchap

## 0.0.9

Released on 2026-03-17.

### Documentation

- Revise documentation structure and installation guides (#30)

### Features

- Improve changed-test selection, discovery, docs, and benchmarks (#18)
- Add doctest support for Python doctests (#32)

### Contributors

- @thejchap

## 0.0.7

Released on 2026-03-09.

### Documentation

- Update README (#9)
- Update README (#10)

### Contributors

- @thejchap
- @dependabot[bot]

## 0.0.6

Released on 2026-03-08.

### Contributors

- @thejchap

## 0.0.5

Released on 2026-03-08.

### Contributors

- @thejchap

## 0.0.4

Released on 2026-02-28.

### Contributors

- @thejchap

## 0.0.3

Released on 2026-02-28.

### Contributors

- @thejchap

## 0.0.2

Released on 2026-02-28.

### Contributors

- @thejchap
