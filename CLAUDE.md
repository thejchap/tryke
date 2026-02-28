# tryke repository

## running clippy

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings

```

## dev guidelines

- All changes must be tested. If you're not testing your changes, you're not done.
- Get your tests to pass. If you didn't run the tests, your code does not work.
- Follow existing code style. Check neighboring files for patterns.
- Always run uvx prek run -a at the end of a task.
- Avoid falling back to patterns that require panic!, unreachable!, or .unwrap().
Instead, try to encode those constraints in the type system.
- Prefer let chains (if let combined with &&) over nested if let statements
to reduce indentation and improve readability.
- If you have to suppress a Clippy lint, prefer to use #[expect()] over [allow()],
where possible.
- Use comments purposefully. Don't use comments to narrate code,
but do use them to explain invariants and why something unusual
was done a particular way.
