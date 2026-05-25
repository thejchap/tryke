## [unreleased]

### Features

- Add configurable discovery cache directory (#128)
- Feat: add clean cache command (#129)

## [0.0.29] - 2026-05-18

### Bug Fixes

- Fix(reporter): honor --collect-only in next, sugar, dot, junit (#115)
- Fix(server): close stale-worker race via in-band did_change RPC (#118)
- Fix multiline CLI diagnostics (#124)

### Documentation

- Docs: add highlights to README, align index.md getting-started with README (#121)

### Features

- Highlight received assertion values (#123)
- Add watch shortcut to clear results (#125)

## [0.0.28] - 2026-05-08

## [0.0.27] - 2026-05-03

## [0.0.26] - 2026-05-02

### Bug Fixes

- Remove interpreter version check from `tryke_runner` (#92)

### Documentation

- Update documentation examples with labeled expectations (#87)

### Features

- Feat(cases): allow @test("label").cases(...) composition (#90)

## [0.0.25] - 2026-04-28

### Features

- Feat(reporter): add `next` and `sugar` reporters (#71)
- Add --no-progress flag to disable terminal native progress bar (#72)

## [0.0.24] - 2026-04-26

### Bug Fixes

- Fix(runner): drain worker stderr to prevent RPC hang on chatty suites (#66)
- Replace importlib.reload with worker subprocess restart (#68)

### Documentation

- Docs: refresh stale tryke test banner version to 0.0.23 (#65)

### Features

- Feat(discovery): restrict walk + skip import graph when paths are passed (#64)
- Feat(hooks): accept Annotated[T, Depends(...)] alongside default-form (#67)

## [0.0.23] - 2026-04-25

## [0.0.22] - 2026-04-25

### Features

- Watch: add --all flag to rerun the full test set on every change (#59)
- Feat(discovery): track PEP 810 lazy imports in dependency graph (#60)
- Feat(discovery): resolve absolute imports across multiple src roots (#61)
- Test labels (#62)

## [0.0.21] - 2026-04-23

## [0.0.20] - 2026-04-23

## [0.0.19] - 2026-04-23

### Documentation

- Docs: correct dot-reporter glyphs and add to_be_instance_of matcher (#53)
- Docs: add session continuity and commit cadence to LLM migration prompt (#55)
- Docs: drop stale _all/before_all/wrap_all hook terminology (#56)

## [0.0.18] - 2026-04-19

## [0.0.17] - 2026-04-18

## [0.0.15] - 2026-04-15

## [0.0.14] - 2026-04-14

## [0.0.13] - 2026-03-29

## [0.0.12] - 2026-03-26

## [0.0.11] - 2026-03-21

## [0.0.10] - 2026-03-17

## [0.0.9] - 2026-03-17

## [0.0.7] - 2026-03-09

## [0.0.6] - 2026-03-08

## [0.0.5] - 2026-03-08

## [0.0.4] - 2026-02-28

## [0.0.3] - 2026-02-28

## [0.0.2] - 2026-02-28
