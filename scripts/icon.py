#!/usr/bin/env python3
"""Draws Skep's icon, and the mark for the menu bar.

Paper rather than darkness, with the app's own orange lying low in one corner
of it and the fine grain the window has, and the comb cell the rail already
uses as the app's own shape. Made by a script rather than by hand so it can be
argued with: every number here is one somebody can change and re-run.

    python3 scripts/icon.py app/skep/assets/skep.icns
    python3 scripts/icon.py /tmp/look.png          # just look at it
    python3 scripts/icon.py --template out.png     # the menu bar mark
"""

import math
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

# Paper, and the one colour on it. The window's light theme is this paper and
# the app's orange is this orange, so the icon and the app are the same two
# things rather than a family resemblance.
PAPER = (0xFB, 0xFA, 0xF8)
ORANGE = (0xFF, 0x7A, 0x2A)

# Apple draws the body of an icon inside a 1024 grid rather than across it.
GRID = 1024
BODY = 824
RADIUS = 185.4
# A superellipse, not a rounded rectangle. The difference is the whole reason
# an Apple icon sits right beside the others in a dock.
SQUIRCLE = 5.0

# How far the light carries, and how much grain is in it. Both quieter than
# they were on a dark ground: orange over paper is already loud, and grain
# that read as texture on black reads as dirt on white.
CARRY = 0.50
GRAIN = 0.030

# The light: where it sits, and how wide it spreads. One bloom low in a corner
# rather than three across the bottom. Three colours need a whole window to
# blend across; an icon is 16 pixels wide often enough that it needs one.
BLOOM = (0.18, 0.94, 0.85)

# The mark. Outlined rather than solid, and this thick because thinner is a
# grey ring at 16 pixels while thicker closes the hole up and becomes a blob.
# Both were drawn and looked at.
ACROSS = 0.215
STROKE = 0.105


def squircle(size, radius, power):
    """A mask the shape of a macOS app icon."""
    mask = Image.new("L", (size * 4, size * 4), 0)
    draw = ImageDraw.Draw(mask)
    half = size * 2
    # r is the corner radius as a fraction of the half width, which is what
    # sets how square the shape stays before it turns.
    n = power
    a = half
    points = []
    for step in range(721):
        t = step * math.pi / 360
        cos, sin = math.cos(t), math.sin(t)
        x = a * math.copysign(abs(cos) ** (2 / n), cos)
        y = a * math.copysign(abs(sin) ** (2 / n), sin)
        points.append((half + x, half + y))
    draw.polygon(points, fill=255)
    return mask.resize((size, size), Image.LANCZOS)


def light(size):
    """Paper with the orange lying low in one corner of it."""
    body = Image.new("RGB", (size, size), PAPER)
    pixels = body.load()
    across_at, down_at, spread = BLOOM
    for y in range(size):
        down = y / size
        for x in range(size):
            away = math.hypot(x / size - across_at, down - down_at) / spread
            if away >= 1:
                pixels[x, y] = PAPER
                continue
            weight = (1 - away) ** 2 * CARRY
            pixels[x, y] = tuple(
                int(max(0, min(255, PAPER[c] + (ORANGE[c] - PAPER[c]) * weight)))
                for c in range(3)
            )
    return body


def grain(image, amount):
    """The same fine tooth the window has, so the two are one material."""
    size = image.size[0]
    noise = Image.effect_noise((size, size), 48).convert("L")
    speckled = image.copy()
    pixels = speckled.load()
    grains = noise.load()
    for y in range(size):
        for x in range(size):
            lift = (grains[x, y] - 128) / 128 * amount * 255
            r, g, b = pixels[x, y]
            pixels[x, y] = (
                int(max(0, min(255, r + lift))),
                int(max(0, min(255, g + lift))),
                int(max(0, min(255, b + lift))),
            )
    return speckled


def cell(size, across=ACROSS, stroke=STROKE):
    """The comb cell, pointy topped, the way the rail draws it."""
    mark = Image.new("L", (size * 4, size * 4), 0)
    draw = ImageDraw.Draw(mark)
    middle = size * 2
    wide = size * 4 * across
    tall = wide * 2 / math.sqrt(3)
    points = [
        (middle, middle - tall),
        (middle + wide, middle - tall / 2),
        (middle + wide, middle + tall / 2),
        (middle, middle + tall),
        (middle - wide, middle + tall / 2),
        (middle - wide, middle - tall / 2),
    ]
    if stroke is None:
        draw.polygon(points, fill=255)
    else:
        draw.polygon(points, outline=255, width=int(size * 4 * stroke))
    return mark.resize((size, size), Image.LANCZOS)


def draw(size):
    body = grain(light(size), GRAIN)
    body.paste(Image.new("RGB", (size, size), ORANGE), (0, 0), cell(size))
    return body


def icon():
    """One full size drawing, cut to the icon's shape and set in its grid."""
    body = draw(BODY)
    shape = squircle(BODY, RADIUS, SQUIRCLE)
    body.putalpha(shape)

    canvas = Image.new("RGBA", (GRID, GRID), (0, 0, 0, 0))
    # A shadow, because every icon beside it in the dock has one.
    shadow = Image.new("RGBA", (GRID, GRID), (0, 0, 0, 0))
    shadow.paste((0, 0, 0, 70), ((GRID - BODY) // 2, (GRID - BODY) // 2 + 12), shape)
    canvas.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(14)))
    canvas.alpha_composite(body, ((GRID - BODY) // 2, (GRID - BODY) // 2))
    return canvas


def template(size=36):
    """The mark alone, for the menu bar.

    Black on nothing, which is what a template image is: macOS throws the
    colour away and redraws it in whatever the menu bar wants, light, dark or
    highlighted. Colour here would be ignored at best and wrong at worst.
    """
    mark = cell(size, across=0.40, stroke=0.15)
    drawn = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    drawn.putalpha(mark)
    return drawn


def main():
    args = sys.argv[1:]
    if args and args[0] == "--template":
        out = Path(args[1] if len(args) > 1 else "skep-menu.png")
        template(36).save(out)
        template(18).save(out.with_name(out.stem.replace("@2x", "") + ".png"))
        print(f"{out} written")
        return
    out = Path(args[0] if args else "skep.icns")
    full = icon()
    if out.suffix == ".png":
        full.save(out)
        print(f"{out} {full.size[0]}x{full.size[1]}")
        return

    with tempfile.TemporaryDirectory() as work:
        iconset = Path(work) / "skep.iconset"
        iconset.mkdir()
        # What iconutil expects, and nothing it does not.
        for size in (16, 32, 128, 256, 512):
            full.resize((size, size), Image.LANCZOS).save(iconset / f"icon_{size}x{size}.png")
            full.resize((size * 2, size * 2), Image.LANCZOS).save(
                iconset / f"icon_{size}x{size}@2x.png"
            )
        subprocess.run(
            ["iconutil", "-c", "icns", str(iconset), "-o", str(out)], check=True
        )
    print(f"{out} written")


if __name__ == "__main__":
    main()
