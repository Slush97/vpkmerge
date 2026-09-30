import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image

REPO = Path("/home/esoc/grimoire-workspace/vpkmerge")
BIN = REPO / "target/release/vpkmerge"
PAK = "/mnt/storage/SteamLibrary/steamapps/common/Deadlock/game/citadel/pak01_dir.vpk"
site = Path(sys.argv[1])
need = [l.strip() for l in Path(sys.argv[2]).read_text().splitlines() if l.strip()]
out = site / "img"
out.mkdir(exist_ok=True)
manifest: dict[str, dict] = {}

def flat(p: str) -> str:
    return p.removeprefix("panorama/images/").replace("/", "__")

for path in need:
    with tempfile.TemporaryDirectory() as tmp:
        r = subprocess.run([BIN, "catalog", "texture", "--vpk", PAK, "--path", path + ".vtex_c", "--thumbs", tmp, "--thumb-size", "4096"], capture_output=True, text=True)
        pngs = list(Path(tmp).glob("*.png"))
        if r.returncode == 0 and pngs:
            im = Image.open(pngs[0])
            big = im.width * im.height > 256 * 256
            name = flat(path) + (".webp" if big else ".png")
            if big:
                im.save(out / name, "WEBP", quality=92, method=6)
            else:
                im.save(out / name, "PNG", optimize=True)
            manifest[path] = {"url": f"img/{name}", "w": im.width, "h": im.height}
            continue
        r = subprocess.run([BIN, "panorama", "dump", "--vpk", PAK, "--out-dir", tmp + "/d", "--prefix", path + ".vsvg_c"], capture_output=True, text=True)
        svgs = list(Path(tmp, "d").rglob("*.vsvg"))
        if svgs and svgs[0].stat().st_size:
            text = svgs[0].read_text()
            m = re.search(r'<svg[^>]*\bwidth="([\d.]+)"[^>]*\bheight="([\d.]+)"', text)
            w, h = (float(m.group(1)), float(m.group(2))) if m else (64.0, 64.0)
            name = flat(path) + ".svg"
            (out / name).write_text(text)
            manifest[path] = {"url": f"img/{name}", "w": w, "h": h}
            continue
    print("NOT FOUND", path)

(site / "assets.json").write_text(json.dumps(manifest, indent=1))
print(len(manifest), "assets")
for k, v in manifest.items():
    print(f"{v['w']:>6}x{v['h']:<6} {v['url']}")
