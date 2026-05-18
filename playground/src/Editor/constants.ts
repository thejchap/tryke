import type { PlaygroundFile } from "./types";

export const DEFAULT_FILES: PlaygroundFile[] = [
  {
    name: "test_math.py",
    source: `from tryke import test, expect

@test
def addition():
    expect(1 + 1).to_equal(2)
    expect(2 + 2).to_equal(4)

@test
def subtraction():
    expect(10 - 3).to_equal(7)
    expect(5 - 5).to_equal(0)

@test
def multiplication():
    expect(3 * 4).to_equal(12)
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
    expect(1 + 1).to_equal(2)
    expect("hello").to_equal("hello")

@test
def truthiness():
    expect(True).to_be_truthy()
    expect(False).to_be_falsy()
    expect(None).to_be_none()

@test
def containers():
    expect([1, 2, 3]).to_contain(2)
    expect({"a": 1}).to_have_key("a")
    expect("hello world").to_contain("world")
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
    expect(1 + 1).to_equal(2)
    expect(2 + 2).to_equal(5)  # fails
    expect(3 + 3).to_equal(6)  # still runs
    expect(4 + 4).to_equal(9)  # still runs
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
    expect(n * n).to_equal(expected)

@test.cases(
    test.case("empty", value=""),
    test.case("hello", value="hello"),
    test.case("spaces", value="  hi  "),
)
def string_strip(value):
    expect(value.strip()).to_equal(value.strip())
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
        expect(1 + 2).to_equal(3)

    @test
    def subtracts():
        expect(5 - 3).to_equal(2)

    with describe("edge cases"):
        @test
        def zero():
            expect(0 + 0).to_equal(0)

        @test
        def negative():
            expect(-1 + -1).to_equal(-2)
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
    expect(double(3)).to_equal(6)
    expect(double(0)).to_equal(0)
    expect(double(-1)).to_equal(-2)

@test
def greets_by_name():
    expect(greet("world")).to_equal("hello, world")
    expect(greet("tryke")).to_equal("hello, tryke")
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

@test
def inserts_user(db=Depends(database)):
    db["users"].append({"name": "alice"})
    expect(db["users"]).to_have_length(1)
    expect(db["connected"]).to_be_truthy()

@test
def admin_exists(user=Depends(admin_user)):
    expect(user["role"]).to_equal("admin")

@test
def config_is_shared(cfg=Depends(config)):
    expect(cfg["debug"]).to_be_truthy()
    expect(cfg["max_retries"]).to_equal(3)
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
    expect(1).to_equal(2)

@test.todo("implement later")
def todo_test():
    pass

@test.xfail("known bug")
def expected_failure():
    expect(1).to_equal(2)

@test
def passing_test():
    expect(True).to_be_truthy()
`,
      },
    ],
  },
];
