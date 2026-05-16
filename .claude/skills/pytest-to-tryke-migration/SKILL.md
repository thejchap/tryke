---
name: pytest-to-tryke-migration
description: Migrate a Python repository from pytest to Tryke under a phased, gated contract — discovery parity, results parity, and CI cutover are explicit checkpoints, not vibes.
---

# pytest → Tryke migration

Tryke (<https://tryke.dev>) is a Rust-based Python test runner with a
Jest-style API: `@test`, `expect(...).to_equal(...)`, `@fixture` +
`Depends()`, `@test.cases`. The conversion cheat sheet lives at
<https://tryke.dev/migration.html>.

## When to use this skill

Activate when **all** of the following hold:

- The target repository has a working pytest suite (pytest is in
  `pyproject.toml` / `requirements*.txt` / `setup.cfg`, or `conftest.py`
  / `test_*.py` files exist).
- The user has asked to migrate to Tryke (or to "use tryke instead of
  pytest").
- The repo is in a clean state — uncommitted local changes get rolled
  into Phase 0 and contaminate the baseline.

If pytest is missing, the suite is already failing, or the user is
asking a one-off question ("how do I write a fixture in tryke?"), do
**not** activate — point them at the cheat sheet instead.

## Goal contract

This skill is a `/goal`-shaped contract. Read this section first; the
phases below are the execution plan that satisfies it.

- **Outcome.** All pytest tests run under `tryke test` with discovery
  and results parity vs the captured pytest baseline, and pytest is
  removed from dev dependencies.
- **Verification surface.** Five gates, in order:
  - **Gate 0** — `pytest --collect-only` has zero collection errors and
    `.migration/pytest-results.xml` exists.
  - **Gate 1** — `tryke test --collect-only` runs without crashing
    (zero collected is fine here).
  - **Gate 3** — discovery parity: the normalized collect lists from
    pytest and tryke match 1:1, modulo the documented `test_` prefix
    strip and any `describe()` group prefixes.
  - **Gate 4** — results parity: per-test pass / fail / skip / xfail
    outcomes match `.migration/pytest-results.xml` after the same
    normalization used for Gate 3.
  - **Gate 5** — CI is green on tryke and pytest is no longer
    installed.
- **Constraints (must not regress).**
  - Do not edit the pytest baseline to paper over a Phase 3 miss. The
    baseline is the source of truth.
  - Do not mass-add `.fatal()` to make Gate 4 numbers match — soft
    assertions are intentional; diagnose each divergence.
  - Do not use `cast()`, `# type: ignore`, `getattr`, or `Any` to
    silence the type checker on `Depends()`. Fix the fixture's return
    type instead.
  - Do not rewrite a `pytest.raises(match=r"...")` regex when moving
    to `to_raise(match=...)` — pass it through unchanged.
  - Do not keep `conftest.py` files "just in case" after re-homing
    their fixtures. Delete them.
- **Boundaries.**
  - Work inside the repo. Create `.migration/` at the repo root for
    baseline and comparison artifacts; add it to `.gitignore`.
  - For large suites, parallelize batches of file conversions across
    sub-agents.
  - Use `tryke test --reporter llm` for any single-test diagnostic
    reruns — its output is tuned for LLM context windows.
- **Iteration policy.** One package / directory at a time. Commit
  **after every file** (or small batch) with a descriptive message and
  push immediately. Update `.migration/CURRENT.md` at the end of each
  session. Append to `.migration/PATTERNS.md` whenever you solve a
  non-obvious conversion the next file is likely to hit too.
- **Blocked stop condition.** Stop and ask the user if a gate fails and
  you cannot trivially explain the mismatch. Stop and ask before
  silencing a typing error on `Depends()` or before mass-adding
  `.fatal()`.

## Working across sessions

A migration of any real size will span multiple sessions. Context
windows are finite, and a fresh session that starts by re-exploring the
tree and re-deriving conversion patterns burns its budget before any
code gets written. Three files in `.migration/` keep sessions
continuous:

- **`.migration/NOTES.md`** — running log of decisions, mismatches,
  flagged-for-human items, and the test counts recorded at each gate.
- **`.migration/PATTERNS.md`** — a concrete, **repo-specific** playbook
  of proven conversions: which conftest fixtures you re-homed and
  where, any project-specific helper wrappers you wrote (e.g. an
  `_expect_raises_async` for awaitable exception assertions), autouse
  rewrites, name-collision aliasing rules, pre-run cleanup commands,
  discovery-tripwire `grep` commands. This is **not** a copy of the
  cheat sheet — it is the adaptations you learned the hard way for
  *this* codebase. When you discover a new pattern mid-session, add it
  here before you forget.
- **`.migration/CURRENT.md`** — a ≤50-line "resume here" pointer:
  current branch and last commit SHA, the exact next file to port with
  its size / test count / any known blockers, copy-paste run and commit
  commands, and a 2–3 item ranked list of what to take on after that. A
  fresh session reads this file first and jumps straight to work.

Update `CURRENT.md` at the end of every session. Append to
`PATTERNS.md` whenever you solve a non-obvious conversion the next file
is likely to hit too.

## Committing and pushing

Commit **after every file** (or small batch of files) with a
descriptive message — do not wait until end of session. After each
commit, push the branch. Two reasons:

1. If a tracking PR exists, every push triggers CI and grows the review
   surface one slice at a time — reviewers can keep up. If no PR exists
   yet, still push: opening a PR later is a single command, but
   unpushed work lost to a crashed session is unrecoverable.
2. When you resume, `git log origin/<branch>` is the unambiguous
   ground-truth for what is done. Local-only commits are invisible to
   the next session.

If a PR is open, note its URL in `CURRENT.md` so the next session
pushes to it automatically.

## Phase -1 — Getting started

Do a scan of the repo and understand the pytest patterns it uses, and
how to convert them to tryke.

If there are functionality gaps, code simple shims.

Make a note of what you encounter and learn in `PATTERNS.md`.

If there are conversion patterns you'd like input on, flag these to the
user before moving forward.

## Phase 0 — Baseline capture (pytest) — Gate 0

On a clean checkout, before any code changes:

1. `pytest --collect-only -q > .migration/pytest-collect.txt 2>&1`
2. `pytest --junit-xml=.migration/pytest-results.xml` (let failures
   surface if any — we just need the XML)
3. Record in `.migration/NOTES.md`:
   - total collected
   - passed / failed / skipped / xfailed / errored counts
   - any collection errors (these **must** be fixed on pytest first —
     do not proceed with a broken baseline)
4. List every active pytest plugin from `pyproject.toml` /
   `requirements*.txt` / `setup.cfg` so we know what behavior we are
   replacing (e.g. `pytest-asyncio`, `pytest-xdist`, `pytest-mock`,
   `pytest-django`).

**Gate 0:** baseline collection has zero errors and results XML exists.

## Phase 1 — Install and configure Tryke — Gate 1

1. Add `tryke` as a dev dependency (`uv add --dev tryke` or the
   project's equivalent). See
   <https://tryke.dev/guides/installation.html>.
2. Add a `[tool.tryke]` section to `pyproject.toml` mirroring the
   pytest `testpaths` / `norecursedirs` values. See
   <https://tryke.dev/guides/configuration.html>.
3. Run `tryke --help` and `tryke test --help` to confirm the binary
   works.

**Gate 1:** `tryke test --collect-only` runs without crashing (it is
fine if it finds **zero** tests at this point — no files have been
converted yet).

## Phase 2 — Mechanical conversion

Convert test files using the cheat sheet at
<https://tryke.dev/migration.html>. Do one package / directory at a
time and keep each conversion small enough to review.

**Display names — use them.** Lift one-line docstrings (or derive a
short phrase from the function name) into `@test("...")`, and label
assertions with `expect(value, "...")` as you go. Tryke surfaces both
in every reporter and discovery extracts them statically — they cost
nothing at runtime, but retrofitting them later is a separate pass
over every test.

**Soft assertions — read this carefully.** Tryke assertions are soft
by default: every `expect()` in a test runs even if an earlier one
fails. See <https://tryke.dev/concepts/soft-assertions.html>. **Do
not** reflexively add `.fatal()` to every assertion to mimic pytest.
Only add `.fatal()` when a later assertion genuinely depends on the
earlier one (e.g. you checked `response.status == 200` and the next
assertions dereference the body).

### Don't

- Do **not** use `cast()`, `# type: ignore`, `getattr`, or `Any` to
  silence the type checker on `Depends()` — fix the fixture's return
  type instead.
- Do **not** translate `pytest.raises(match=r"...")` by rewriting the
  regex — pass it through unchanged on `to_raise(match=...)`.
- Do **not** keep `conftest.py` files "just in case" after moving the
  fixtures. Delete them.
- Do **not** paper over a missing test in Phase 3 by editing the
  baseline file. The baseline is the source of truth.

## Phase 3 — Discovery parity gate — Gate 3

1. `tryke test --collect-only > .migration/tryke-collect.txt`
2. Normalize both lists and diff them:
   - pytest: `test_file.py::test_name[case_label]`
   - tryke:  `test_file.py::name[case_label]`
   - The only expected differences are the `test_` prefix stripping
     and the describe-group prefixes where you added `with
     describe(...)` blocks.
3. For each test present in pytest's list but missing from Tryke's,
   diagnose in this order:
   1. Dynamic imports in the module or a transitive import
      (`grep -R importlib.import_module`). Replace with static imports.
   2. Fixture still living in `conftest.py` — move it module-local.
   3. `@test.cases` label that is not a string literal, or case kwargs
      that don't match the function signature.
   4. A plain `def test_foo` that never got decorated with `@test`.
   5. A test nested inside an `if`/`for`/`while` body — flatten.
4. For each test present in Tryke's list but missing from pytest's,
   you probably double-collected a `describe()` group. Fix the
   decorator.

**Gate 3:** the two collect lists match 1:1 (modulo the documented
prefix changes). Record the final count in `.migration/NOTES.md`.

## Phase 4 — Results parity gate — Gate 4

1. `tryke test --reporter junit > .migration/tryke-results.xml`
2. Compare per-test outcomes against `.migration/pytest-results.xml`.
   The JUnit `<testcase>` names do **not** match byte-for-byte —
   Phase 2 stripped the `test_` prefix and Phase 3 may have added
   `describe()` group prefixes. Apply the same normalization you used
   to pass Gate 3 before comparing:
   - Strip the leading `test_` from pytest names
     (`test_file.py::test_add` → `test_file.py::add`).
   - For any test you moved under a `with describe("group"):` block,
     prepend `group::` on the pytest side (or strip it on the Tryke
     side) so the IDs line up.
   - Parametrize labels (`[case_label]`) already match between
     systems — do not rewrite them.

   Then for each normalized name, compare pass / fail / skip / xfail
   status.
3. For each divergence:
   - Rerun the single test with the LLM-friendly reporter:
     `tryke test -k <name> --reporter llm`
     (see <https://tryke.dev/guides/reporters.html>). Feed that output
     back through this skill to diagnose.
   - Common causes, in order of likelihood:
     - Wrong assertion matcher (`to_be` vs `to_equal`, `to_contain`
       vs `to_have_length`, forgotten `.not_`).
     - A fixture's teardown ran at a different scope than pytest's.
     - A soft-assertion cascade: an assertion that would have
       short-circuited under pytest now runs and fails on a `None`
       the earlier assertion was supposed to guard. Add `.fatal()` on
       the guarding assertion.
     - `pytest.raises(match=...)` regex was rewritten instead of
       copied.
4. Do not mass-add `.fatal()` to make numbers match — diagnose each
   case.

**Gate 4:** per-test outcomes match the pytest baseline. Record the
final counts in `.migration/NOTES.md`.

## Phase 5 — Cleanup — Gate 5

1. Remove `pytest`, `pytest-asyncio`, `pytest-xdist`, `pytest-mock`
   (if the usage was mechanical), and other pytest-only plugins from
   dev dependencies. Leave anything whose functionality Tryke does not
   yet provide and flag it in `.migration/NOTES.md`.
2. Delete empty `conftest.py` files.
3. Update CI to call `tryke test --reporter junit` (or `--reporter
   llm` if CI logs are consumed by an LLM) in place of `pytest`.
4. Update the project README's testing section.
5. Delete `.migration/` once you have committed the migration.

**Gate 5:** CI is green on Tryke; pytest is no longer installed.

## Reporting back

After each gate, summarize in chat:

- which files changed in this phase
- the current test counts (collected / passed / failed / skipped /
  xfailed)
- anything you had to skip or flag for human review

## Using with `/goal` (Codex)

This skill is shaped as a `/goal` contract: the **Goal contract**
section above maps 1:1 to the Codex goal template (outcome,
verification surface, constraints, boundaries, iteration policy,
blocked stop). To run the migration under a Codex goal, paste:

```
/goal Migrate this repository from pytest to Tryke under the
pytest-to-tryke-migration skill at
.claude/skills/pytest-to-tryke-migration/SKILL.md. Done when Gates 0
through 5 all pass: pytest baseline captured cleanly, tryke
configured, discovery parity matches, per-test results parity
matches the captured pytest JUnit XML, and pytest is removed from
dev dependencies with CI green on tryke. Work one package at a
time; commit and push after every file; maintain .migration/NOTES.md,
PATTERNS.md, and CURRENT.md across sessions. Use `tryke test
--reporter llm` for any single-test diagnostic rerun. Stop and ask if
a gate fails and the mismatch is non-trivial, if a typing error on
Depends() would require a cast/ignore to silence, or if you would
need to mass-add .fatal() to satisfy Gate 4.
```

Use `/goal` to inspect status, pause, resume, or clear. Between
checkpoints, the agent should report a compact status: current
gate, what's verified, what's left, and any blockers.
