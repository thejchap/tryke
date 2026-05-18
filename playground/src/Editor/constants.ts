import type { PlaygroundFile } from "./types";

export const DEFAULT_FILES: PlaygroundFile[] = [
  {
    name: "test_math.py",
    source: `from tryke import test, expect

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
`,
  },
];

export interface Example {
  label: string;
  files: PlaygroundFile[];
}

export const EXAMPLES: Example[] = [
  {
    label: "Basic assertions",
    files: [
      {
        name: "test_basics.py",
        source: `from tryke import test, expect

@test
def equality():
    expect(1 + 1, name="integer addition").to_equal(2)
    expect("hello", name="string identity").to_equal("hello")

@test
def truthiness():
    expect(True, name="true is truthy").to_be_truthy()
    expect(False, name="false is falsy").to_be_falsy()
    expect(None, name="none is none").to_be_none()

@test
def containers():
    expect([1, 2, 3], name="list contains").to_contain(2)
    expect({"a": 1}, name="dict has key").to_have_key("a")
    expect("hello world", name="string contains").to_contain("world")
`,
      },
    ],
  },
  {
    label: "Soft assertions",
    files: [
      {
        name: "test_soft.py",
        source: `from tryke import test, expect

@test
def multiple_checks():
    """All assertions run even if earlier ones fail."""
    expect(1 + 1, name="one plus one").to_equal(2)
    expect(2 + 2, name="two plus two").to_equal(5)  # fails
    expect(3 + 3, name="three plus three").to_equal(6)  # still runs
    expect(4 + 4, name="four plus four").to_equal(9)  # still runs
`,
      },
    ],
  },
  {
    label: "Parametrized cases",
    files: [
      {
        name: "test_cases.py",
        source: `from tryke import test, expect

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
`,
      },
    ],
  },
  {
    label: "Describe blocks",
    files: [
      {
        name: "test_describe.py",
        source: `from tryke import test, expect, describe

with describe("Calculator"):
    @test
    def adds():
        expect(1 + 2, name="sum").to_equal(3)

    @test
    def subtracts():
        expect(5 - 3, name="difference").to_equal(2)

    with describe("edge cases"):
        @test
        def adding_zeros():
            expect(0 + 0, name="zero sum").to_equal(0)

        @test
        def negative_numbers():
            expect(-1 + -1, name="negative sum").to_equal(-2)
`,
      },
    ],
  },
  {
    label: "Multi-file imports",
    files: [
      {
        name: "helpers.py",
        source: `def double(n):
    return n * 2

def greet(name):
    return f"hello, {name}"
`,
      },
      {
        name: "test_helpers.py",
        source: `from tryke import test, expect
from helpers import double, greet

@test
def doubles_numbers():
    expect(double(3), name="double 3").to_equal(6)
    expect(double(0), name="double 0").to_equal(0)
    expect(double(-1), name="double -1").to_equal(-2)

@test
def greets_by_name():
    expect(greet("world"), name="greet world").to_equal("hello, world")
    expect(greet("tryke"), name="greet tryke").to_equal("hello, tryke")
`,
      },
    ],
  },
  {
    label: "Fixtures & Depends",
    files: [
      {
        name: "test_fixtures.py",
        source: `from typing import Annotated
from tryke import test, expect, fixture, Depends

@fixture
def database():
    """Per-test database connection."""
    db = {"users": [], "connected": True}
    yield db
    db["connected"] = False

@fixture
def admin_user(db: Annotated[dict, Depends(database)]):
    """Creates an admin in the database fixture."""
    user = {"name": "admin", "role": "admin"}
    db["users"].append(user)
    return user

@fixture(per="scope")
def config():
    """Shared config — created once, reused across tests."""
    return {"debug": True, "max_retries": 3}

@test
def inserts_user(db: Annotated[dict, Depends(database)]):
    db["users"].append({"name": "alice"})
    expect(db["users"], name="user list").to_have_length(1)
    expect(db["connected"], name="db connection").to_be_truthy()

@test
def admin_exists(user: Annotated[dict, Depends(admin_user)]):
    expect(user["role"], name="admin role").to_equal("admin")

@test
def config_is_shared(cfg: Annotated[dict, Depends(config)]):
    expect(cfg["debug"], name="debug flag").to_be_truthy()
    expect(cfg["max_retries"], name="max retries").to_equal(3)
`,
      },
    ],
  },
  {
    label: "Kitchen Sink",
    files: [
      {
        name: "test_kitchen_sink.py",
        source: `from typing import Annotated, Callable
from tryke import test, expect, describe, fixture, Depends
from mathlib import add, multiply, divide, clamp


# --- Fixtures with dependency injection ---

@fixture
def numbers():
    """Fresh list of numbers for each test."""
    return [1, 2, 3, 4, 5]


@fixture(per="scope")
def config():
    """Shared config — created once across all tests."""
    return {"precision": 2, "max_value": 100}


@fixture
def clamped_add(cfg: Annotated[dict, Depends(config)]):
    """A helper that adds and clamps to max_value."""
    def _add(a, b):
        return clamp(add(a, b), 0, cfg["max_value"])
    return _add


# --- Describe blocks for grouping ---

with describe("arithmetic"):
    @test
    def test_add():
        expect(add(2, 3), name="2 + 3").to_equal(5)
        expect(add(-1, 1), name="-1 + 1").to_equal(0)

    @test
    def test_multiply():
        expect(multiply(3, 4), name="3 * 4").to_equal(12)
        expect(multiply(0, 99), name="0 * 99").to_equal(0)

    with describe("division"):
        @test
        def test_divide():
            expect(divide(10, 2), name="10 / 2").to_equal(5.0)

        @test
        def test_divide_zero():
            try:
                divide(1, 0)
                expect(True, name="should have raised").to_be_falsy()
            except ValueError as e:
                expect(str(e), name="error message").to_equal("division by zero")


# --- Parametrized cases ---

@test.cases(
    test.case("low", value=-5, expected=0),
    test.case("in range", value=50, expected=50),
    test.case("high", value=200, expected=100),
)
def test_clamp(value, expected):
    expect(clamp(value, 0, 100), name="clamped value").to_equal(expected)


# --- Fixtures in action ---

@test
def uses_number_list(nums: Annotated[list, Depends(numbers)]):
    expect(nums, name="numbers list").to_have_length(5)
    expect(nums, name="contains 3").to_contain(3)
    nums.append(6)
    expect(nums, name="after append").to_have_length(6)


@test
def clamped_addition(do_add: Annotated[Callable, Depends(clamped_add)]):
    expect(do_add(50, 30), name="50 + 30 clamped").to_equal(80)
    expect(do_add(99, 99), name="99 + 99 clamped").to_equal(100)


# --- Markers ---

@test.skip("not implemented yet")
def future_feature():
    pass


@test.todo("pending design review")
def new_api():
    pass
`,
      },
      {
        name: "mathlib.py",
        source: `"""A small math library used by the test file."""


def add(a, b):
    return a + b


def multiply(a, b):
    return a * b


def divide(a, b):
    if b == 0:
        raise ValueError("division by zero")
    return a / b


def clamp(value, low, high):
    return max(low, min(value, high))
`,
      },
    ],
  },
  {
    label: "Skip / Todo / XFail",
    files: [
      {
        name: "test_markers.py",
        source: `from tryke import test, expect

@test.skip("not ready yet")
def skipped_test():
    expect(1, name="should not run").to_equal(2)

@test.todo("implement later")
def todo_test():
    pass

@test.xfail("known bug")
def expected_failure():
    expect(1, name="known wrong").to_equal(2)

@test
def passing_test():
    expect(True, name="always true").to_be_truthy()
`,
      },
    ],
  },
];
