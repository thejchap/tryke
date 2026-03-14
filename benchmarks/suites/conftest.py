import pytest

def pytest_configure(config):
    config.addinivalue_line(
        "filterwarnings",
        "ignore::pytest.PytestCollectionWarning",
    )
