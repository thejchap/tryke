from tryke import expect, test


@test
def addition():
    expect(1 + 1, name="one plus one").to_equal(2)
    expect(2 + 2, name="two plus two").to_equal(4)


@test
def subtraction():
    expect(10 - 3, name="ten minus three").to_equal(7)
    expect(5 - 5, name="five minus five").to_equal(0)


@test
def multiplication():
    expect(3 * 4, name="three times four").to_equal(12)
