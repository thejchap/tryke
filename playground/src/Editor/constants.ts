import type { PlaygroundFile } from "./types";

export const DEFAULT_FILES: PlaygroundFile[] = [
  {
    name: "test_math.py",
    source: `from tryke import test, expect

@test(name="addition")
def addition():
    expect(1 + 1, name="one plus one").to_equal(2)
    expect(2 + 2, name="two plus two").to_equal(4)

@test(name="subtraction")
def subtraction():
    expect(10 - 3, name="ten minus three").to_equal(7)
    expect(5 - 5, name="five minus five").to_equal(0)

@test(name="multiplication")
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

@test(name="equality checks")
def equality():
    expect(1 + 1, name="integer addition").to_equal(2)
    expect("hello", name="string identity").to_equal("hello")

@test(name="truthiness checks")
def truthiness():
    expect(True, name="true is truthy").to_be_truthy()
    expect(False, name="false is falsy").to_be_falsy()
    expect(None, name="none is none").to_be_none()

@test(name="container checks")
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

@test(name="multiple checks")
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
    name="square",
)
def square(n, expected):
    expect(n * n, name="squared value").to_equal(expected)

@test.cases(
    test.case("empty", value=""),
    test.case("hello", value="hello"),
    test.case("spaces", value="  hi  "),
    name="string strip",
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
    @test(name="adds two numbers")
    def adds():
        expect(1 + 2, name="sum").to_equal(3)

    @test(name="subtracts two numbers")
    def subtracts():
        expect(5 - 3, name="difference").to_equal(2)

    with describe("edge cases"):
        @test(name="adding zeros")
        def zero():
            expect(0 + 0, name="zero sum").to_equal(0)

        @test(name="negative numbers")
        def negative():
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

@test(name="doubles numbers")
def doubles_numbers():
    expect(double(3), name="double 3").to_equal(6)
    expect(double(0), name="double 0").to_equal(0)
    expect(double(-1), name="double -1").to_equal(-2)

@test(name="greets by name")
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
        source: `from tryke import test, expect, fixture, Depends

@fixture
def database():
    """Per-test database connection."""
    db = {"users": [], "connected": True}
    yield db
    db["connected"] = False

@fixture
def admin_user(db=Depends(database)):
    """Creates an admin in the database fixture."""
    user = {"name": "admin", "role": "admin"}
    db["users"].append(user)
    return user

@fixture(per="scope")
def config():
    """Shared config — created once, reused across tests."""
    return {"debug": True, "max_retries": 3}

@test(name="inserts user into database")
def inserts_user(db=Depends(database)):
    db["users"].append({"name": "alice"})
    expect(db["users"], name="user list").to_have_length(1)
    expect(db["connected"], name="db connection").to_be_truthy()

@test(name="admin exists")
def admin_exists(user=Depends(admin_user)):
    expect(user["role"], name="admin role").to_equal("admin")

@test(name="config is shared")
def config_is_shared(cfg=Depends(config)):
    expect(cfg["debug"], name="debug flag").to_be_truthy()
    expect(cfg["max_retries"], name="max retries").to_equal(3)
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

@test.skip("not ready yet", name="skipped test")
def skipped_test():
    expect(1, name="should not run").to_equal(2)

@test.todo("implement later", name="todo test")
def todo_test():
    pass

@test.xfail("known bug", name="expected failure")
def expected_failure():
    expect(1, name="known wrong").to_equal(2)

@test(name="passing test")
def passing_test():
    expect(True, name="always true").to_be_truthy()
`,
      },
    ],
  },
];
