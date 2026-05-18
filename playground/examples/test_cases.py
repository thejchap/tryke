from tryke import expect, test


@test.cases(
    test.case("zero", n=0, expected=0),
    test.case("positive", n=3, expected=9),
    test.case("negative", n=-2, expected=4),
)
def square(n, expected):
    expect(n * n, name="squared value").to_equal(expected)


@test.cases(
    test.case("empty", value=""),
    test.case("hello", value="hello"),
    test.case("spaces", value="  hi  "),
)
def string_strip(value):
    expect(value.strip(), name="stripped value").to_equal(value.strip())
