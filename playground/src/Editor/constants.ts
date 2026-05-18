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
