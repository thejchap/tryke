from tryke import expect, test


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
