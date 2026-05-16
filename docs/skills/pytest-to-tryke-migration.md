# pytest → Tryke migration skill

A Claude Code [Skill][skills] (also pasteable as a Codex
[`/goal`][goals]) that walks an AI coding assistant through a
**phased, gated** migration from pytest to Tryke. It pairs with
`tryke test --reporter llm` for concise, structured failure
diagnostics tuned to LLM context windows.

The skill encodes a `/goal`-shaped contract — outcome, verification
surface, constraints, boundaries, iteration policy, and an explicit
blocked-stop condition — so the assistant has a real finish line
instead of a vibe. The five gates (baseline capture, tryke install,
**discovery parity**, **results parity**, CI cutover) catch the
silent failure modes that ruin migrations: tests that stopped being
collected, or assertions that quietly inverted.

[skills]: https://docs.claude.com/en/docs/agents/skills
[goals]: https://developers.openai.com/codex/use-cases/follow-goals

## Install

Drop the canonical SKILL.md into your project's `.claude/skills/`
directory:

```bash
mkdir -p .claude/skills/pytest-to-tryke-migration
curl -fsSL https://raw.githubusercontent.com/thejchap/tryke/main/.claude/skills/pytest-to-tryke-migration/SKILL.md \
  -o .claude/skills/pytest-to-tryke-migration/SKILL.md
```

Commit the file alongside your other project tooling so every fresh
session picks it up.

## Invoke (Claude Code)

Open Claude Code in the repo and ask:

> Migrate this repo from pytest to tryke.

The skill auto-activates on the migration intent + the pytest
fingerprint (a `conftest.py`, `test_*.py` files, or `pytest` in
`pyproject.toml` / `requirements*.txt` / `setup.cfg`). The assistant
will run the phases, write artifacts under `.migration/`, and stop at
each gate to confirm parity.

## Invoke (Codex `/goal`)

The skill's **Goal contract** section maps 1:1 to the Codex goal
template. Paste this as your goal:

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

Use `/goal` to inspect status, pause, resume, or clear between
checkpoints.

### Strong vs weak goals

The `/goal` documentation makes a sharp distinction:

- **Weak.** `/goal migrate this repo to tryke` — no verification
  surface, no stop condition, no constraints. The agent decides
  what "done" means and you find out at the end.
- **Strong.** The template above — names the five gates as the
  verification surface, pins down what must not regress, and gives
  the agent an unambiguous blocked-stop condition.

Treat the strong template as a starting point. Add repo-specific
constraints (e.g. "do not touch `tests/integration/` — that suite
already uses tryke") inline before pasting.

## What the skill contains

A summary of the SKILL.md sections, in order:

1. **When to use this skill** — activation triggers.
2. **Goal contract** — outcome, verification surface (Gates 0–5),
   constraints, boundaries, iteration policy, blocked stop condition.
3. **Working across sessions** — the three `.migration/` files
   (`NOTES.md`, `PATTERNS.md`, `CURRENT.md`) that keep multi-session
   migrations continuous.
4. **Committing and pushing** — commit-after-every-file cadence and
   why local-only work is invisible to the next session.
5. **Phases -1 → 5** — the executable plan. Each phase names its
   gate.
6. **Reporting back** — what to summarize after each gate.
7. **Using with `/goal` (Codex)** — the copyable strong-goal
   template.

See the full file on GitHub:
<https://github.com/thejchap/tryke/blob/main/.claude/skills/pytest-to-tryke-migration/SKILL.md>.

## See also

- [Migration cheat sheet](../migration.md) — the conversion reference
  the skill points the assistant at for mechanical translations.
- [LLM reporter](../guides/reporters.md#llm) — the reporter the skill
  uses for single-test diagnostic reruns at Gate 4.
- [Soft assertions](../concepts/soft-assertions.md) — why the skill
  forbids reflexive `.fatal()`.
