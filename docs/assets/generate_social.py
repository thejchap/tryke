"""Generate the 1200x630 social card used in link previews.

Run via ``uvx --from pillow python docs/assets/generate_social.py`` when the
logo or copy changes. Keeps the build step out of CI while making the
artifact regenerable.
"""

from __future__ import annotations

import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ASSETS = Path(__file__).parent
OUT = ASSETS / "social.png"
LOGO = ASSETS / "logo.png"

# Dark teal / slate to match the site's primary palette.
BG = (15, 23, 30)
FG = (245, 250, 250)
ACCENT = (94, 204, 196)
SUB = (170, 190, 200)

SIZE = (1200, 630)

TITLE = "Tryke"
TAGLINE = "A Rust-based Python test runner"
SUBLINE = "with a Jest-style API"
URL = "tryke.dev"


def _font(names: list[str], size: int) -> ImageFont.FreeTypeFont | ImageFont.ImageFont:
    for name in names:
        try:
            return ImageFont.truetype(name, size)
        except OSError:
            continue
    return ImageFont.load_default()


def main() -> int:
    img = Image.new("RGB", SIZE, BG)
    draw = ImageDraw.Draw(img)

    if LOGO.exists():
        logo = Image.open(LOGO).convert("RGBA")
        # Fit logo into a 360px square on the left.
        target = 360
        logo.thumbnail((target, target), Image.Resampling.LANCZOS)
        lx = 90
        ly = (SIZE[1] - logo.height) // 2
        img.paste(logo, (lx, ly), logo)

    title_font = _font(
        ["DejaVuSans-Bold.ttf", "Arial Bold.ttf", "Helvetica-Bold.ttf"], 120
    )
    tag_font = _font(["DejaVuSans.ttf", "Arial.ttf", "Helvetica.ttf"], 36)
    url_font = _font(["DejaVuSansMono.ttf", "Menlo.ttf", "Courier New.ttf"], 28)

    text_x = 500
    draw.text((text_x, 180), TITLE, font=title_font, fill=FG)
    draw.text((text_x, 330), TAGLINE, font=tag_font, fill=SUB)
    draw.text((text_x, 380), SUBLINE, font=tag_font, fill=SUB)
    draw.text((text_x, 500), URL, font=url_font, fill=ACCENT)

    img.save(OUT, optimize=True)
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
