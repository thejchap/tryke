from tryke import expect, test


@test(name="coverage")
def test_coverage() -> None:
    expect("a", "a is truthy").to_be_truthy()
