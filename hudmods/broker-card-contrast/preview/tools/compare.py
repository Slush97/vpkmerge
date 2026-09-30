import json
import subprocess
import sys
from pathlib import Path

import numpy as np
from PIL import Image

# Serve the preview dir first: (cd .. && python3 -m http.server 8766 --bind 127.0.0.1), with a
# fonts/ symlink to Deadlock/game/citadel/panorama/fonts.  usage: compare.py CARD SCALE TAG [EXTRA_QUERY]
P = Path(__file__).resolve().parent
SITE = P.parent
card, scale, tag = sys.argv[1], sys.argv[2], sys.argv[3]
extra = sys.argv[4] if len(sys.argv) > 4 else ""
url = f"http://127.0.0.1:8766/calib.html?card={card}&scale={scale}{extra}"
shot = P / f"render_{tag}.png"
subprocess.run(["chromium", "--headless=new", "--disable-gpu", "--hide-scrollbars", "--window-size=900,1500",
                "--virtual-time-budget=9000", f"--screenshot={shot}", url], capture_output=True)
dom = subprocess.run(["chromium", "--headless=new", "--disable-gpu", "--window-size=900,1500", "--virtual-time-budget=9000",
                      "--dump-dom", url], capture_output=True, text=True).stdout
info = json.loads(dom.split('<pre id="missing">')[1].split("</pre>")[0].replace("&quot;", '"'))
x, y, w, h = info["card"]
render = np.asarray(Image.open(shot).convert("RGB"), dtype=np.float64)
ref_img = Image.open(SITE / {"frenzy": "ref/frenzy.png", "focuslens": "ref/focus_lens.png"}[card]).convert("RGB")
ref = np.asarray(ref_img, dtype=np.float64)
rh, rw, _ = ref.shape
best = None
for dy in range(-12, 16):
    for dx in range(-12, 16):
        oy, ox = int(round(y)) + dy, int(round(x)) + dx
        if oy < 0 or ox < 0 or oy + rh > render.shape[0] or ox + rw > render.shape[1]:
            continue
        crop = render[oy:oy + rh, ox:ox + rw]
        err = np.abs(crop - ref)[40:-40, 40:-40].mean()
        if best is None or err < best[0]:
            best = (err, ox, oy)
err, ox, oy = best
crop = render[oy:oy + rh, ox:ox + rw]
side = np.concatenate([ref, crop, np.clip(np.abs(crop - ref) * 3, 0, 255)], axis=1).astype(np.uint8)
Image.fromarray(side).save(P / f"cmp_{tag}.png")
print(json.dumps({"mean_abs_err": round(err, 2), "offset_in_card": [ox - x, oy - y], "card": info["card"], "missing": info["missing"], "fonts": info["fonts"]}))
