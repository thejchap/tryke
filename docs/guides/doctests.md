# Doctests

tryke automatically discovers and runs Python doctests alongside your regular tests. No configuration needed.

## How discovery works

tryke's static parser scans docstrings for the `>>>` prompt marker. Any docstring containing interactive examples is collected as a test. This works at every level:

### Module-level

```python
"""
Module for math utilities.

>>> add(1, 2)
3
"""

def add(a, b):
    return a + b
```

### Function-level

```python
def greet(name):
    """
    Return a greeting.

    >>> greet("world")
    'hello, world'
    """
    return f"hello, {name}"
```

### Class-level

```python
class Counter:
    """
    A simple counter.

    >>> c = Counter(0)
    >>> c.value
    0
    """

    def __init__(self, value):
        self.value = value
```

### Method-level

```python
class Counter:
    def __init__(self, value):
        self.value = value

    def increment(self):
        """
        Increment the counter by one.

        >>> c = Counter(0)
        >>> c.increment()
        >>> c.value
        1
        """
        self.value += 1
```

## ELLIPSIS support

Doctests run with the `ELLIPSIS` flag enabled by default. Use `...` to match variable output:

```python
def now():
    """
    Return the current timestamp.

    >>> now()  # doctest: +ELLIPSIS
    datetime.datetime(...)
    """
    import datetime
    return datetime.datetime.now()
```

## Running doctests

Doctests are collected automatically with `tryke test`. They appear in output with a `doctest:` prefix:

```bash
tryke test
```

You can filter them like any other test:

```bash
tryke test -k "doctest"
```

## Static discovery

Like all tryke test discovery, doctests are found by parsing source files — not by importing modules. This means discovery is fast and side-effect free. See [test discovery](../concepts/discovery.md) for details.
