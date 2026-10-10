import json
import re
import sys
from pathlib import Path

import numpy as np
from PIL import Image

site = Path(sys.argv[1])
K = float(sys.argv[2]) if len(sys.argv) > 2 else 1.0
man = json.loads((site / "assets.json").read_text())
(site / "img" / "lin").mkdir(exist_ok=True)

def lin(c: np.ndarray) -> np.ndarray:
    c = c / 255.0
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)

def lin_hex(m: re.Match) -> str:
    h = m.group(1)
    if len(h) == 3:
        h = "".join(ch * 2 for ch in h)
    rgb = np.array([int(h[i:i + 2], 16) for i in (0, 2, 4)], float)
    out = "".join(f"{int(round(v)):02x}" for v in lin(rgb) ** (1 / K) * 255)
    return "#" + out + (h[6:8] if len(h) == 8 else "")

for path, a in man.items():
    src = site / a["url"]
    name = Path(a["url"]).name
    if "_mask_" in name:
        a["lin"] = a["url"]
        continue
    if name.endswith(".svg"):
        text = src.read_text()
        text = re.sub(r"#([0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{3})\b", lin_hex, text)
        (site / "img" / "lin" / name).write_text(text)
    else:
        im = Image.open(src).convert("RGBA")
        arr = np.asarray(im, float)
        arr[..., :3] = np.round(lin(arr[..., :3]) ** (1 / K) * 255)
        out = Image.fromarray(arr.astype(np.uint8), "RGBA")
        if name.endswith(".webp"):
            out.save(site / "img" / "lin" / name, "WEBP", lossless=True, quality=100, method=6)
        else:
            out.save(site / "img" / "lin" / name, "PNG", optimize=True)
    a["lin"] = f"img/lin/{name}"
(site / "assets.json").write_text(json.dumps(man, indent=1))
print("linearized", len(man))
