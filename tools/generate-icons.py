#!/usr/bin/env python3
"""Draw the menu bar artwork.

Run from the repository root:

    python3 tools/generate-icons.py

Writes into src-tauri/icons/:

    tray-claude.png     the resting icon
    tray-alert-{0,1,2}.png   the mark plus three dots, cycled while blocked

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
import struct
import zlib
from pathlib import Path

ICONS = Path(__file__).resolve().parent.parent / "src-tauri" / "icons"

LOGICAL = 18          # a status item renders about this tall, in points
SCALE = 2             # draw at 2x for a clean 2:1 downscale
SUPERSAMPLE = 8       # 8x8 samples per pixel, so edges get 64 alpha levels

HEIGHT = LOGICAL * SCALE
PAD_RIGHT = 7 * SCALE     # gap between the artwork and the counts
GAP = 7 * SCALE           # gap between the mark and the first dot

CLAY = (0xD9, 0x77, 0x57)         # the mark
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


def coverage(shape, x, y):
    hits = 0.0
    for sy in range(SUPERSAMPLE):
        for sx in range(SUPERSAMPLE):
            hits += shape(x + (sx + 0.5) / SUPERSAMPLE, y + (sy + 0.5) / SUPERSAMPLE)
    return hits / (SUPERSAMPLE * SUPERSAMPLE)


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


if __name__ == "__main__":
    main()
