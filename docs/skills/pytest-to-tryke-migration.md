# pytest → Tryke migration skill

A Claude Code [Skill][skills] that converts **one pytest test file**
(or a small package) to Tryke and verifies the converted file's
outcomes match what pytest produced for it.

The skill is the unit of work, not the whole migration. Repo-level
orchestration — baseline capture, iteration across files, whole-suite
parity gates, removing pytest from dev deps — lives in a Codex
[`/goal`][goals] that invokes the skill once per file.

[skills]: https://docs.claude.com/en/docs/agents/skills
[goals]: https://developers.openai.com/codex/use-cases/follow-goals

## Install

```bash
mkdir -p .claude/skills/pytest-to-tryke-migration
curl -fsSL https://raw.githubusercontent.com/thejchap/tryke/main/.claude/skills/pytest-to-tryke-migration/SKILL.md \
  -o .claude/skills/pytest-to-tryke-migration/SKILL.md
```

Commit the file alongside your other project tooling so every fresh
session picks it up.

## Invoke per-file (Claude Code)

Point Claude Code at a specific file:

> Migrate `tests/test_widgets.py` from pytest to tryke.

The skill activates, applies the conversions, and verifies that the
file's tryke discovery and outcomes match what pytest produced. Repeat
file by file, committing after each.

## Drive the whole migration with `/goal`

For a full-repo migration, paste this as a Codex goal. It captures a
baseline, iterates file-by-file invoking the skill, and stops when
discovery + outcomes parity is satisfied and pytest is removed:

```text
/goal Migrate this repository from pytest to tryke. Use the
pytest-to-tryke-migration skill once per test file.

Before starting:
- Capture baseline: `pytest --collect-only -q > .migration/baseline-collect.txt`
  and `pytest --junit-xml=.migration/baseline-results.xml`. Add
  `.migration/` to .gitignore.
- Install tryke as a dev dep and add a `[tool.tryke]` section
  mirroring the pytest testpaths.

Iterate: for each test file under the configured testpaths, invoke the
skill, then commit and push. Use sub-agents to parallelize batches if
the suite is large.

Done when:
- `tryke test --collect-only` matches the baseline 1:1, modulo the
  `test_` prefix strip and any added describe() group prefixes.
- `tryke test --reporter junit` per-test outcomes match
  .migration/baseline-results.xml after the same normalization.
- pytest and its mechanically-replaced plugins (pytest-asyncio,
  pytest-xdist, pytest-mock) are removed from dev deps.
- CI calls `tryke test` and is green.

Stop and ask if: a file's converted discovery or outcomes diverge in
ways the skill's Step 3 troubleshooting can't explain; a Depends()
typing error would require cast/ignore to silence; or you would need
to mass-add .fatal() to satisfy the outcome parity check.
```

### Strong vs weak goals

The [follow-goals][goals] docs emphasize a finish line the agent can
verify. Compare:

- **Weak.** `/goal migrate this repo to tryke` — no verification
  surface, no stop condition. The agent decides what "done" means.
- **Strong.** The template above — names the baseline as the
  verification surface, defines what must be true at done, and gives
  the agent an unambiguous blocked-stop condition.

Edit the template with repo-specific constraints before pasting (e.g.
"skip `tests/integration/` — it already uses tryke").

## See also

- [Migration cheat sheet](../migration.md) — the conversion reference
  the skill points at for mechanical translations.
- [LLM reporter](../guides/reporters.md#llm) — the reporter the skill
  uses for single-test diagnostic reruns when an outcome diverges.
- [Soft assertions](../concepts/soft-assertions.md) — why the skill
  forbids reflexive `.fatal()`.
