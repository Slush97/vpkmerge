"""WCAG contrast of the corrupted-item (Broker) tooltip text, stock vs. this override.

Decodes the three corrupted tooltip backers from pak01, composites every pixel of
each card region through the panel layers the text sits on, then composites the text
color on top. Reports the 1st-percentile ratio (99% of the card area does at least
this well); `!` marks a WCAG 2.x AA failure (4.5:1 body text, 3:1 large text).

The "after" colors and SCRIM mirror corrupted_contrast.vcss; keep them in sync.

    python3 check_contrast.py /path/to/citadel/pak01_dir.vpk
"""

import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image

REPO = Path(__file__).resolve().parents[2]
THEMES = ["weapon", "vitality", "spirit"]
BACKER = "panorama/images/tooltips/items/tooltip_backer_{}_corrupted_psd.vtex_c"

SCRIM = {"weapon": "#1C0C06A6", "vitality": "#0A1610A6", "spirit": "#110E22A6"}

Paint = tuple[str, float]

# (label, region, layers under the text, stock (color, opacity), override, large text?)
ROWS: list[tuple[str, str, list[str], Paint, Paint, bool]] = [
    ("item name", "head", [], ("#FFFFFF", 1), ("#FFFFFF", 1), True),
    ("CORRUPTED stamp", "head", [], ("#F3FFAA", 1), ("#F3FFAA", 1), False),
    ("description", "body", [], ("#CDCDCD", 1), ("#CDCDCD", 1), False),
    ("description .highlight", "body", [], ("#F8F8F8", 1), ("#F8F8F8", 1), False),
    ("description .diminish", "body", [], ("#BFBBB090", 1), ("#CFCBC0", 1), False),
    ("description weapon hl", "body", [], ("#EC9719", 1), ("#EC9719", 1), False),
    ("description spirit hl", "body", [], ("#CE90FF", 1), ("#CE90FF", 1), False),
    ("description vitality hl", "body", [], ("#7BBA1D", 1), ("#7BBA1D", 1), False),
    ("description .isNegative", "body", [], ("#FF6A6A", 1), ("#FF6A6A", 1), False),
    ("stat name", "body", ["#00000066"], ("#FFFFFF", 0.5), ("#FFFFFF", 0.85), False),
    ("stat unit / prefix", "body", ["#00000066"], ("#FFFFFF50", 1), ("#FFFFFFCC", 1), False),
    ("stat value +", "body", ["#00000066", "#D8EC5720"], ("#FFEFD7", 1), ("#FFEFD7", 1), False),
    ("stat value -", "body", ["#00000066", "#FF410D30"], ("#FF8F8F", 1), ("#FF8F8F", 1), False),
    ("headline stat type", "body", ["#D8EC5720"], ("#FFFFFF", 1), ("#FFFFFF", 1), False),
    ("innate RANDOMIZED tag", "body", ["#FF410D30"], ("#FF410D", 1), ("#FF9A9A", 1), False),
    ("innate penalty value", "body", ["#FF410D30"], ("#FF8F8F", 1), ("#FF8F8F", 1), False),
    ("components label", "foot", ["#10130DEA"], ("#FFFFFF50", 1), ("#FFFFFFC0", 1), False),
    ("component name", "foot", ["#10130D70"], ("#FFEFD790", 1), ("#FFEFD7", 1), False),
]

REGIONS = {"head": (0.02, 0.14), "body": (0.14, 0.86), "foot": (0.86, 0.97)}


def hexc(s: str) -> tuple[np.ndarray, float]:
    s = s.lstrip("#")
    rgb = np.array([int(s[i : i + 2], 16) for i in (0, 2, 4)], dtype=np.float64) / 255
    return rgb, (int(s[6:8], 16) / 255 if len(s) == 8 else 1.0)


def luminance(c: np.ndarray) -> np.ndarray:
    lin = np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)
    return 0.2126 * lin[..., 0] + 0.7152 * lin[..., 1] + 0.0722 * lin[..., 2]


def over(color: str, bg: np.ndarray, opacity: float = 1.0) -> np.ndarray:
    rgb, a = hexc(color)
    a *= opacity
    return rgb * a + bg * (1 - a)


def contrast_p1(fg: np.ndarray, bg: np.ndarray) -> float:
    lf, lb = luminance(fg), luminance(bg)
    ratio = (np.maximum(lf, lb) + 0.05) / (np.minimum(lf, lb) + 0.05)
    return float(np.percentile(ratio, 1))


def decode_backers(pak: Path, out: Path) -> dict[str, np.ndarray]:
    backers = {}
    for theme in THEMES:
        entry = BACKER.format(theme)
        proc = subprocess.run(
            [
                "cargo", "run", "-q", "--release",
                "--manifest-path", str(REPO / "Cargo.toml"), "-p", "vpkmerge-cli", "--",
                "catalog", "texture", "--vpk", str(pak), "--path", entry,
                "--thumbs", str(out), "--thumb-size", "4096",
            ],
            capture_output=True,
            text=True,
        )
        if proc.returncode != 0:
            sys.exit(f"decoding {entry} failed:\n{proc.stderr}")
        png = out / (entry.removesuffix(".vtex_c").replace("/", "__") + ".png")
        backers[theme] = np.asarray(Image.open(png).convert("RGB"), dtype=np.float64) / 255
    return backers


def surface(backer: np.ndarray, theme: str, where: str, layers: list[str], fixed: bool) -> np.ndarray:
    h, w, _ = backer.shape
    y0, y1 = REGIONS[where]
    bg = backer[int(h * y0) : int(h * y1), int(w * 0.06) : int(w * 0.94)].reshape(-1, 3)
    if fixed and where == "body":
        bg = over(SCRIM[theme], bg)
    for layer in layers:
        bg = over(layer, bg)
    return bg


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    with tempfile.TemporaryDirectory() as tmp:
        backers = decode_backers(Path(sys.argv[1]), Path(tmp))

    print(f"{'text':26s}" + "".join(f"{t:>17s}" for t in THEMES) + "   (stock -> override)")
    fails = {False: 0, True: 0}
    for label, where, layers, stock, override, large in ROWS:
        need = 3.0 if large else 4.5
        row = f"{label:26s}"
        for theme in THEMES:
            cells = []
            for fixed, (color, opacity) in ((False, stock), (True, override)):
                bg = surface(backers[theme], theme, where, layers, fixed)
                ratio = contrast_p1(over(color, bg, opacity), bg)
                fails[fixed] += ratio < need
                cells.append(f"{ratio:4.1f}{'!' if ratio < need else ' '}")
            row += f"   {cells[0]}->{cells[1]}"
        print(row)
    total = len(ROWS) * len(THEMES)
    print(f"\nAA failures: stock {fails[False]}/{total}, override {fails[True]}/{total}")
    return 1 if fails[True] else 0


if __name__ == "__main__":
    sys.exit(main())
