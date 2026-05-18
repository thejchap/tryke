from helpers import double, greet
from tryke import expect, test


@test
def doubles_numbers():
    expect(double(3), name="double 3").to_equal(6)
    expect(double(0), name="double 0").to_equal(0)
    expect(double(-1), name="double -1").to_equal(-2)


@test
def greets_by_name():
    expect(greet("world"), name="greet world").to_equal("hello, world")
    expect(greet("tryke"), name="greet tryke").to_equal("hello, tryke")
