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

# Paper, and the three colours the window's light is made of. The app's light
# theme is this paper and these are its three, so the icon and the app are the
# same things rather than a family resemblance to each other.
PAPER = (0xFB, 0xFA, 0xF8)
ORANGE = (0xFF, 0x7A, 0x2A)
ROSE = (0xFF, 0x3D, 0x6E)
BLUE = (0x4B, 0x5C, 0xFF)

# Apple draws the body of an icon inside a 1024 grid rather than across it.
GRID = 1024
BODY = 824
RADIUS = 185.4
# A superellipse, not a rounded rectangle. The difference is the whole reason
# an Apple icon sits right beside the others in a dock.
SQUIRCLE = 5.0

# How much grain is in the light. Quieter than the window's: grain that reads
# as texture on black reads as dirt on white.
GRAIN = 0.030

# The light: where each colour sits, how wide it spreads, and how strong it
# is. Low and left to right, the way the three lie across the bottom of the
# window. The orange is the app's own and leads; the other two are there to be
# noticed second, and were drawn at four strengths to find where that is.
# Louder than this and the corner where rose meets blue goes grey.
BLOOMS = [
    (0.18, 0.94, 0.85, 0.50, ORANGE),
    (0.55, 1.00, 0.72, 0.28, ROSE),
    (0.94, 0.88, 0.70, 0.18, BLUE),
]

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
    """Paper, with the three lying low across one corner of it."""
    body = Image.new("RGB", (size, size), PAPER)
    pixels = body.load()
    for y in range(size):
        down = y / size
        for x in range(size):
            across = x / size
            colour = list(PAPER)
            for (at_x, at_y, spread, strength, tint) in BLOOMS:
                away = math.hypot(across - at_x, down - at_y) / spread
                if away >= 1:
                    continue
                weight = (1 - away) ** 2 * strength
                for channel in range(3):
                    colour[channel] += (tint[channel] - colour[channel]) * weight
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


def state(size, kind, across=0.41, stroke=0.15):
    """One menu bar state, black on nothing.

    That is what a template image is: macOS throws the colour away and redraws
    the shape in whatever the bar wants, light, dark, or inverted while a menu
    is open over it. Colour here is ignored at best and wrong at worst.

    The stroke is lighter than the icon's. The icon's ratio drawn at eighteen
    points leaves a hole too small to see, so hollow and solid stop being two
    different things, and telling them apart is the entire job up there.
    """
    scale = size * 8
    mark = Image.new("L", (scale, scale), 0)
    pen = ImageDraw.Draw(mark)
    middle = scale // 2
    wide = scale * across
    tall = wide * 2 / math.sqrt(3)
    points = [
        (middle, middle - tall),
        (middle + wide, middle - tall / 2),
        (middle + wide, middle + tall / 2),
        (middle, middle + tall),
        (middle - wide, middle + tall / 2),
        (middle - wide, middle - tall / 2),
    ]
    if kind == "running":
        pen.polygon(points, fill=255)
    else:
        pen.polygon(points, outline=255, width=int(scale * stroke))
        if kind == "working":
            # Filled from the left, the way the symbol it replaces was.
            filled = Image.new("L", (scale, scale), 0)
            ImageDraw.Draw(filled).polygon(points, fill=255)
            left = Image.new("L", (scale, scale), 0)
            ImageDraw.Draw(left).rectangle([0, 0, middle, scale], fill=255)
            mark.paste(255, (0, 0), Image.composite(filled, left.point(lambda _: 0), left))

    drawn = Image.new("RGBA", (scale, scale), (0, 0, 0, 0))
    drawn.putalpha(mark)
    return drawn.resize((size, size), Image.LANCZOS)


def main():
    args = sys.argv[1:]
    if args and args[0] == "--menu":
        # Eighteen points is what the menu bar gives an image, drawn at two
        # times so it stays sharp on a retina display.
        into = Path(args[1] if len(args) > 1 else ".")
        into.mkdir(parents=True, exist_ok=True)
        for kind in ("idle", "running", "working"):
            state(36, kind).save(into / f"{kind}.png")
        print(f"{into}/*.png written")
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
