# tryke repository

## running clippy

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings

```

## running python tests

```bash
cargo run test --reporter llm
```

## running rust tests

we use [`cargo-nextest`](https://nexte.st) — install once with
`cargo install cargo-nextest --locked`, then:

```bash
cargo nextest run --workspace --all-features
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
- in python, always prefer strong, precise typing. use concrete types, typed
protocols, dataclasses, NamedTuple, or TypedDict over `object` / `Any` / untyped
dict. fix the underlying typing problem rather than reaching for `# noqa`,
`# type: ignore`, `cast()`, `getattr()`/`hasattr()`, or other dynamic escape
hatches. if a type checker complains, the first move is to tighten the types,
not silence the checker.
- use comments purposefully. don't use comments to narrate code,
but do use them to explain invariants and why something unusual
was done a particular way.

## style guidelines

- comments and docs should be capitalized: <https://google.github.io/styleguide/cppguide.html#Punctuation_Spelling_and_Grammar> and <https://peps.python.org/pep-0008/#comments>
