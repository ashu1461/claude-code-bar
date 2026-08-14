#!/usr/bin/env python3
"""Draw the menu bar artwork.

Run from the repository root:

    python3 tools/generate-icons.py

Writes into src-tauri/icons/:

    tray-claude.png          the resting menu bar icon
    tray-alert-{0,1,2}.png   the mark plus three dots, cycled while blocked
    icon-source.png          the app icon at 1024, and the .icns built from it

The app icon is the same sunburst on a rounded tile, so the icon in Finder and
the item in the menu bar are recognisably the same thing. It is indigo rather
than Claude's clay: this is not an Anthropic application, and the colour makes
that clear at a glance.

Three things here were learnt the hard way and are easy to undo by accident:

1.  **Draw at exactly twice the rendered height.** A status item renders about
    18pt tall. Anything else means a fractional downscale, which is what made
    an earlier version look pixelated on a non-retina display.

2.  **Keep the strokes thick and few.** Finely tapered rays collapse to about
    one pixel at this size and turn to mush. Eight rounded strokes meeting at
    the centre survive; twelve tapered ones did not.

3.  **Never animate with transparency.** The menu bar is dark for most people,
    and a 25%-alpha red dot on a dark background is invisible — you see one
    dot instead of three. The dots are fully opaque and the highlight moves by
    changing colour, which reads on light and dark alike.

Padding is baked into the image because a status item gives you none of its
own, and without it the counts sit right against the artwork.
"""

import math
import shutil
import struct
import subprocess
import zlib
from pathlib import Path

ICONS = Path(__file__).resolve().parent.parent / "src-tauri" / "icons"

LOGICAL = 18          # a status item renders about this tall, in points
SCALE = 2             # draw at 2x for a clean 2:1 downscale
SUPERSAMPLE = 8       # 8x8 samples per pixel, so edges get 64 alpha levels

HEIGHT = LOGICAL * SCALE
PAD_RIGHT = 7 * SCALE     # gap between the artwork and the counts
GAP = 12 * SCALE          # gap between the mark and the first dot

APP_TILE = (0x3B, 0x5B, 0xDB)     # app icon background
APP_MARK = (0xFF, 0xFF, 0xFF)     # app icon sunburst
APP_SIZE = 1024
APP_CORNER = 0.225                # fraction of the tile, matching macOS
APP_OUTER = 0.30                  # sunburst radius, fraction of the tile
APP_STROKE = 0.040                # stroke half-width, fraction of the tile
APP_SAMPLES = 2                   # 1024px is big enough that 2x2 is ample

# The menu bar mark, matching the app icon. A touch lighter than the app
# tile, because a saturated indigo goes muddy against a dark menu bar.
CLAY = (0x5C, 0x7C, 0xFA)
DOT_LIT = (0xFF, 0x6B, 0x6B)      # the travelling highlight
DOT_DIM = (0xD0, 0x3C, 0x3C)      # the other two, still fully opaque

RAYS = 8
OUTER = 0.44 * HEIGHT
HALF_WIDTH = 0.058 * HEIGHT

DOT_RADIUS = 2.5 * SCALE
DOT_STEP = 6.0 * SCALE

# Which dot is lit in each frame. Cycling these reads as movement.
FRAMES = [(0,), (1,), (2,)]


def write_png(path, width, height, pixels):
    raw = bytearray()
    for y in range(height):
        raw.append(0)
        for x in range(width):
            raw.extend(pixels[y * width + x])

    def chunk(tag, data):
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


def distance_to_segment(px, py, ax, ay, bx, by):
    vx, vy = bx - ax, by - ay
    wx, wy = px - ax, py - ay
    length = vx * vx + vy * vy
    t = 0.0 if length == 0 else max(0.0, min(1.0, (wx * vx + wy * vy) / length))
    return math.hypot(px - (ax + t * vx), py - (ay + t * vy))


def in_mark(px, py, cx, cy):
    """Rounded strokes radiating from the centre, so the hub stays solid."""
    for i in range(RAYS):
        angle = i * 2 * math.pi / RAYS
        end_x = cx + OUTER * math.cos(angle)
        end_y = cy + OUTER * math.sin(angle)
        if distance_to_segment(px, py, cx, cy, end_x, end_y) <= HALF_WIDTH:
            return 1.0
    return 0.0


def in_disc(px, py, cx, cy, radius):
    return 1.0 if (px - cx) ** 2 + (py - cy) ** 2 <= radius * radius else 0.0


def in_rounded_square(px, py, size, radius):
    dx = max(radius - px, 0, px - (size - radius))
    dy = max(radius - py, 0, py - (size - radius))
    return 1.0 if dx * dx + dy * dy <= radius * radius else 0.0


def in_mark_sized(px, py, centre, outer, half_width):
    for i in range(RAYS):
        angle = i * 2 * math.pi / RAYS
        end_x = centre + outer * math.cos(angle)
        end_y = centre + outer * math.sin(angle)
        if distance_to_segment(px, py, centre, centre, end_x, end_y) <= half_width:
            return 1.0
    return 0.0


def build_app_icon():
    """The 1024px source, plus the .icns and PNGs the bundler wants."""
    size = APP_SIZE
    centre = size / 2
    outer = size * APP_OUTER
    half = size * APP_STROKE
    corner = size * APP_CORNER

    pixels = []
    for y in range(size):
        for x in range(size):
            tile = coverage(
                lambda a, b: in_rounded_square(a, b, size, corner), x, y, APP_SAMPLES
            )
            if tile <= 0:
                pixels.append((0, 0, 0, 0))
                continue
            on_mark = coverage(
                lambda a, b: in_mark_sized(a, b, centre, outer, half), x, y, APP_SAMPLES
            )
            pixels.append(
                (
                    round(APP_TILE[0] * (1 - on_mark) + APP_MARK[0] * on_mark),
                    round(APP_TILE[1] * (1 - on_mark) + APP_MARK[1] * on_mark),
                    round(APP_TILE[2] * (1 - on_mark) + APP_MARK[2] * on_mark),
                    round(255 * tile),
                )
            )

    source = ICONS / "icon-source.png"
    write_png(source, size, size, pixels)
    print(f"icon-source.png      {size}x{size}")

    # Everything below is macOS tooling; the app is macOS-only anyway.
    iconset = ICONS / "icon.iconset"
    if iconset.exists():
        shutil.rmtree(iconset)
    iconset.mkdir()
    for px, name in [
        (16, "icon_16x16"), (32, "icon_16x16@2x"),
        (32, "icon_32x32"), (64, "icon_32x32@2x"),
        (128, "icon_128x128"), (256, "icon_128x128@2x"),
        (256, "icon_256x256"), (512, "icon_256x256@2x"),
        (512, "icon_512x512"), (1024, "icon_512x512@2x"),
    ]:
        subprocess.run(
            ["sips", "-z", str(px), str(px), str(source), "--out", str(iconset / f"{name}.png")],
            check=True, capture_output=True,
        )
    subprocess.run(
        ["iconutil", "-c", "icns", str(iconset), "-o", str(ICONS / "icon.icns")],
        check=True, capture_output=True,
    )
    shutil.rmtree(iconset)

    for px, name in [(512, "icon.png"), (32, "32x32.png"), (128, "128x128.png"), (256, "128x128@2x.png")]:
        subprocess.run(
            ["sips", "-z", str(px), str(px), str(source), "--out", str(ICONS / name)],
            check=True, capture_output=True,
        )
    print("icon.icns and bundle PNGs rebuilt")


def coverage(shape, x, y, samples=SUPERSAMPLE):
    hits = 0.0
    for sy in range(samples):
        for sx in range(samples):
            hits += shape(x + (sx + 0.5) / samples, y + (sy + 0.5) / samples)
    return hits / (samples * samples)


def main():
    ICONS.mkdir(parents=True, exist_ok=True)
    centre = HEIGHT / 2
    mark = lambda x, y: in_mark(x, y, centre, centre)  # noqa: E731

    resting_width = int(HEIGHT + PAD_RIGHT)
    pixels = [
        (*CLAY, round(255 * coverage(mark, x, y)))
        for y in range(HEIGHT)
        for x in range(resting_width)
    ]
    write_png(ICONS / "tray-claude.png", resting_width, HEIGHT, pixels)
    print(f"tray-claude.png      {resting_width}x{HEIGHT}")

    dot_centres = [HEIGHT + GAP + i * DOT_STEP for i in range(3)]
    alert_width = int(dot_centres[-1] + DOT_RADIUS + PAD_RIGHT)

    for index, lit in enumerate(FRAMES):
        pixels = []
        for y in range(HEIGHT):
            for x in range(alert_width):
                on_mark = coverage(mark, x, y)
                if on_mark > 0:
                    pixels.append((*CLAY, round(255 * on_mark)))
                    continue
                colour, alpha = DOT_DIM, 0.0
                for dot, dot_x in enumerate(dot_centres):
                    hit = coverage(
                        lambda px, py, dx=dot_x: in_disc(px, py, dx, centre, DOT_RADIUS),
                        x,
                        y,
                    )
                    if hit > alpha:
                        alpha = hit
                        colour = DOT_LIT if dot in lit else DOT_DIM
                pixels.append((*colour, round(255 * alpha)))
        write_png(ICONS / f"tray-alert-{index}.png", alert_width, HEIGHT, pixels)
    print(f"tray-alert-{{0,1,2}}.png {alert_width}x{HEIGHT}")

    build_app_icon()


if __name__ == "__main__":
    main()
