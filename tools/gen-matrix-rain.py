"""Seamlessly looping Matrix code rain, 256x256, 1024 frames.
Loop math: drop pos = (p0 + v*f) mod L with v = m*L/FRAMES (integer m),
so pos(FRAMES) == pos(0). Glyph churn period divides FRAMES. No seam."""
import random, sys, os
from PIL import Image, ImageDraw, ImageFont

OUT = sys.argv[1]
COLOR = sys.argv[2] if len(sys.argv) > 2 else "green"
SIZE, FRAMES = 256, 1024
CELL = 10
COLS = SIZE // CELL          # 25
ROWS = SIZE // CELL + 2      # 27 (a bit of overhang)
TAIL = 14                    # fade length in cells

if COLOR == "green":
    HEAD, BODY = (190, 255, 190), (0, 255, 70)
else:  # red, RUINER-style
    HEAD, BODY = (255, 200, 190), (255, 30, 20)

random.seed(1999)
font = ImageFont.truetype("/usr/share/fonts/TTF/JetBrainsMono-Bold.ttf", CELL + 2)
GLYPHS = "0123456789$+*=<>|:;#@%&?!^~ZTKXR"

cols = []
for c in range(COLS):
    L = random.randint(34, 60)            # cycle length in cells
    m = random.randint(5, 11)             # full cycles per loop
    v = m * L / FRAMES                    # cells per frame, seamless by construction
    p0 = random.uniform(0, L)
    churn = random.choice([8, 16, 32])    # glyph change period (divides FRAMES)
    phases = [random.randrange(FRAMES) for _ in range(ROWS)]
    cols.append((L, v, p0, churn, phases))

def glyph(c, r, f, churn, phase):
    t = ((f + phase) // churn) % (FRAMES // churn)
    return GLYPHS[hash((c, r, t)) % len(GLYPHS)]

os.makedirs(OUT, exist_ok=True)
for f in range(FRAMES):
    im = Image.new("RGB", (SIZE, SIZE), (0, 0, 0))
    d = ImageDraw.Draw(im)
    for c, (L, v, p0, churn, phases) in enumerate(cols):
        head = (p0 + v * f) % L
        for k in range(TAIL):
            row_pos = head - k
            r = int(row_pos % L)
            if r >= ROWS:
                continue
            fade = (1.0 - k / TAIL) ** 1.6
            col = HEAD if k == 0 else tuple(int(ch * fade) for ch in BODY)
            d.text((c * CELL, r * CELL - CELL), glyph(c, r, f, churn, phases[r % ROWS]),
                   fill=col, font=font)
    im.save(f"{OUT}/f_{f:05d}.png")
    if f % 256 == 0:
        print("frame", f)
print("done:", FRAMES, "frames")
