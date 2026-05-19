from tryke import expect, test


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
    expect({"a": 1}, name="dict has key").to_contain("a")
    expect("hello world", name="string contains").to_contain("world")
