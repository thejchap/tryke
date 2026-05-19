"""Code coverage logic

https://docs.python.org/3/library/sys.monitoring.html
"""

import sys
from types import CodeType
from typing import Final

mon = sys.monitoring
TOOL_ID = mon.COVERAGE_ID
TOOL_NAME: Final = "tryke"

type HitLocation = tuple[
    # Path name
    str,
    # Line number
    int,
]


class Coverage:
    def __init__(self) -> None:
        self._hits: set[HitLocation] = set()

    def register(self) -> None:
        existing_tool = mon.get_tool(TOOL_ID)
        if existing_tool:
            if existing_tool != TOOL_NAME:
                msg = f"Unexpected existing tool: {existing_tool}"
                raise ValueError(msg)
        else:
            mon.use_tool_id(TOOL_ID, TOOL_NAME)
            mon.register_callback(TOOL_ID, mon.events.LINE, self._line_callback)
            mon.set_events(TOOL_ID, mon.events.LINE)

    def _line_callback(self, code: CodeType, line_number: int) -> object:
        self._hits.add((code.co_filename, line_number))
        return mon.DISABLE
