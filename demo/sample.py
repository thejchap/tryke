from typing import Annotated

import tryke as t


# Fixtures with `per="scope"` are cached for their scope:
# - Module-level for globally defined fixtures
# - Per `describe` block for fixtures defined within a `describe` block
@t.fixture(per="scope")
def database():
    db = {}
    yield db
    db.clear()


with t.describe("users"):
    # By default, fixtures run per-test
    # Fixtures can be composed by requesting other fixtures
    @t.fixture
    def users(database: Annotated[dict[str, dict[str, str]], t.Depends(database)]):
        database["users"] = {}
        return database["users"]

    with t.describe("get"):
        # Define display labels for tests
        # Async tests are supported
        @t.test("returns a stored user")
        async def test_get(users: Annotated[dict[str, str], t.Depends(users)]):
            users["alice"] = "alice@example.com"
            # Pass a label as the second argument to expect() so reports
            # show "returns stored email" instead of the raw expression
            t.expect(users["alice"], "returns stored email").to_equal(
                "alice@example.com"
            )

    with t.describe("set"):

        @t.test("stores a new user")
        async def test_set(users: Annotated[dict[str, str], t.Depends(users)]):
            users["bob"] = "bob@example.com"
            t.expect(users["bob"], "stores email under user key").to_equal(
                "bob@example.com"
            )
