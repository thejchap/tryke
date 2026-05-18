from typing import Annotated

from tryke import Depends, expect, fixture, test


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
