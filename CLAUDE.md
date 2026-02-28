# tryke repository

## running clippy

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings

```

## dev guidelines

- all changes must be tested. if you're not testing your changes, you're not done.
- get your tests to pass. if you didn't run the tests, your code does not work.
- follow existing code style. check neighboring files for patterns.
- always run uvx prek run -a at the end of a task.
- avoid falling back to patterns that require panic!, unreachable!, or .unwrap().
Instead, try to encode those constraints in the type system.
- prefer let chains (if let combined with &&) over nested if let statements
to reduce indentation and improve readability.
- if you have to suppress a clippy lint, prefer to use #[expect()] over [allow()],
where possible.
- use comments purposefully. don't use comments to narrate code,
but do use them to explain invariants and why something unusual
was done a particular way.
