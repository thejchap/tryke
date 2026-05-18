from tryke import expect, test


@test
def multiple_checks():
    """All assertions run even if earlier ones fail."""
    expect(1 + 1, name="one plus one").to_equal(2)
    expect(2 + 2, name="two plus two").to_equal(5)  # fails
    expect(3 + 3, name="three plus three").to_equal(6)  # still runs
    expect(4 + 4, name="four plus four").to_equal(9)  # still runs
