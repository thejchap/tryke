# Soft assertions

In tryke, assertions are **soft by default**. Every assertion in a test runs even if earlier ones fail. This is the opposite of pytest, where the first failing `assert` stops the test.

## Why soft assertions

With hard assertions, you fix one failure at a time:

```python
# pytest — only sees the first failure per run
def test_response():
    assert response.status == 200   # fails, test stops here
    assert "ok" in response.body    # never runs
    assert "json" in response.type  # never runs
```

With soft assertions, you see everything that's wrong in a single run:

```python
from tryke import expect, test

@test
def response_check():
    expect(response.status).to_equal(200)     # fails, continues
    expect(response.body).to_contain("ok")    # runs, may also fail
    expect(response.type).to_contain("json")  # runs, may also fail
```

This is especially valuable when tests are slow (network calls, database setup) or when failures tend to cluster.

## Per-assertion diagnostics

Each soft assertion produces its own diagnostic output showing the expected and actual values. When multiple assertions fail, you see all of them in the test report — not just the first one.

Soft assertions apply **per-case** for [parametrized tests](cases.md). Each case runs all its assertions independently; a failure inside one case never short-circuits the next case.

## `.fatal()` for early stopping

Sometimes continuing after a failure doesn't make sense. Chain `.fatal()` to stop the test immediately:

```python
@test
def check_response():
    expect(response.status).to_equal(200).fatal()  # stop if wrong status
    expect(response.body).to_contain("ok")          # only runs if status was 200
    expect(response.headers).to_contain("json")     # only runs if status was 200
```

Use `.fatal()` when later assertions depend on an earlier one passing — like checking a status code before inspecting the body.

## Comparison with pytest

| Behavior | pytest | tryke |
|----------|--------|-------|
| Default | Hard — first failure stops the test | Soft — all assertions run |
| Early stop | Default behavior | Opt-in via `.fatal()` |
| Diagnostic output | One failure per test per run | All failures per test per run |
