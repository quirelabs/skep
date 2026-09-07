#!/usr/bin/env python3
"""Draws Skep's icon out of the same material as its window.

The window is lit by three colours lying low behind a fine grain; this is that
same light, turned into a corner so it reads at 32 pixels, with the comb cell
the rail already uses as the app's own shape. Made by a script rather than by
hand so it can be argued with: every number here is a number somebody can
change and re-run.

    python3 scripts/icon.py app/skep/assets/skep.icns
"""

import math
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

# The window's own palette. Base first, then the three the light is made of.
BASE = (0x0B, 0x0B, 0x0C)
SKY = [(0xFF, 0x7A, 0x2A), (0xFF, 0x3D, 0x6E), (0x4B, 0x5C, 0xFF)]
INK = (0xFF, 0xFF, 0xFF)

# Apple draws the body of an icon inside a 1024 grid rather than across it.
GRID = 1024
BODY = 824
RADIUS = 185.4
# A superellipse, not a rounded rectangle. The difference is the whole reason
# an Apple icon sits right beside the others in a dock.
SQUIRCLE = 5.0

# How far the light carries, and how much grain is in it. Stronger than the
# window's: an icon is looked at from further away and has no text to protect.
CARRY = 0.85
GRAIN = 0.055


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
    """The three colours, gathered into one corner and bled into each other."""
    body = Image.new("RGB", (size, size), BASE)
    pixels = body.load()
    # Each is a soft round bloom: where it sits, how wide, how strong.
    blooms = [
        (0.16, 0.92, 0.78, 1.00),  # the warm one, low and to the left
        (0.62, 1.04, 0.66, 0.85),
        (1.02, 0.52, 0.70, 0.62),  # cool, off the right edge
    ]
    for y in range(size):
        down = y / size
        for x in range(size):
            across = x / size
            colour = list(BASE)
            for (cx, cy, spread, strength) in blooms:
                away = math.hypot(across - cx, down - cy) / spread
                if away >= 1:
                    continue
                fade = (1 - away) ** 2
                weight = fade * strength * CARRY
                for c in range(3):
                    colour[c] += (SKY[blooms.index((cx, cy, spread, strength))][c] - colour[c]) * weight
            pixels[x, y] = tuple(int(max(0, min(255, v))) for v in colour)
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


def cell(size):
    """The comb cell, pointy topped, the way the rail draws it.

    Solid rather than outlined. The rail's glyph is a thin outline because it
    sits at 20 pixels beside words; an icon has to survive 16 pixels in a
    Spotlight result with nothing beside it, and at that size an outline is a
    grey ring. Tried both: the outline is mud and this is unmistakable.
    """
    mark = Image.new("L", (size * 4, size * 4), 0)
    draw = ImageDraw.Draw(mark)
    middle = size * 2
    across = size * 4 * 0.215
    down = across * 2 / math.sqrt(3)
    points = [
        (middle, middle - down),
        (middle + across, middle - down / 2),
        (middle + across, middle + down / 2),
        (middle, middle + down),
        (middle - across, middle + down / 2),
        (middle - across, middle - down / 2),
    ]
    draw.polygon(points, fill=255)
    return mark.resize((size, size), Image.LANCZOS)


def draw(size):
    body = grain(light(size), GRAIN)
    mark = cell(size)
    ink = Image.new("RGB", (size, size), INK)
    body.paste(ink, (0, 0), mark)
    return body


def icon():
    """One full size drawing, cut to the icon's shape and set in its grid."""
    body = draw(BODY)
    shape = squircle(BODY, RADIUS, SQUIRCLE)
    body.putalpha(shape)

    canvas = Image.new("RGBA", (GRID, GRID), (0, 0, 0, 0))
    # A shadow, because every icon beside it in the dock has one.
    shadow = Image.new("RGBA", (GRID, GRID), (0, 0, 0, 0))
    shadow.paste((0, 0, 0, 90), ((GRID - BODY) // 2, (GRID - BODY) // 2 + 12), shape)
    canvas.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(14)))
    canvas.alpha_composite(body, ((GRID - BODY) // 2, (GRID - BODY) // 2))
    return canvas


def main():
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "skep.icns")
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
