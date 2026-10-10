"""Seamlessly looping "time vortex" spiral frames (Paradox palette).
Rotation phase advances by exactly 2*pi over FRAMES and every phase term
uses integer harmonics, so frame FRAMES-1 flows into frame 0 with no seam.
usage: gen-time-vortex.py OUT_DIR [SIZE=512] [FRAMES=64]"""
import os
import sys

import numpy as np
from PIL import Image

OUT = sys.argv[1]
SIZE = int(sys.argv[2]) if len(sys.argv) > 2 else 512
FRAMES = int(sys.argv[3]) if len(sys.argv) > 3 else 64

# Paradox: deep blue-black base, teal arms, magenta hot edge, gold sparks.
STOPS = np.array(
    [
        [8, 6, 20],      # near-black navy
        [10, 60, 80],    # deep teal
        [27, 190, 199],  # paradox teal
        [235, 40, 130],  # magenta
        [255, 200, 90],  # gold
    ],
    dtype=np.float64,
)

def palette(t):
    """t in [0,1] -> RGB via piecewise-linear gradient."""
    t = np.clip(t, 0.0, 1.0) * (len(STOPS) - 1)
    i = np.minimum(t.astype(int), len(STOPS) - 2)
    frac = (t - i)[..., None]
    return STOPS[i] * (1 - frac) + STOPS[i + 1] * frac

ys, xs = np.mgrid[0:SIZE, 0:SIZE]
cx = (xs - SIZE / 2 + 0.5) / (SIZE / 2)
cy = (ys - SIZE / 2 + 0.5) / (SIZE / 2)
r = np.hypot(cx, cy) + 1e-6
theta = np.arctan2(cy, cx)
lr = np.log(r)

os.makedirs(OUT, exist_ok=True)
for f in range(FRAMES):
    ph = 2 * np.pi * f / FRAMES
    # Three-arm log spiral sweeping inward, plus a counter-rotating fine swirl
    # and a radial pulse. All phase multipliers are integers: seamless.
    v1 = np.sin(3 * theta + 4.0 * lr - 2 * ph)
    v2 = np.sin(7 * theta - 6.0 * lr + 3 * ph)
    pulse = np.sin(9.0 * lr + 1 * ph)
    t = 0.52 + 0.34 * v1 + 0.17 * v2 * (0.5 + 0.5 * pulse)
    # brighten the vortex eye, darken the far corners
    t = t * (1.0 - 0.35 * np.clip(r - 0.9, 0, 1)) + 0.25 * np.exp(-14 * r * r)
    rgb = palette(t).astype(np.uint8)
    Image.fromarray(rgb, "RGB").save(f"{OUT}/f_{f:05d}.png")
print(f"done: {FRAMES} frames {SIZE}x{SIZE} -> {OUT}")
