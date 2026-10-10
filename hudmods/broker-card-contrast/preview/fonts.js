// Deadlock's Panorama fonts (game/citadel/panorama/fonts). They are licensed with the game,
// so the page never ships them: they load from a local folder or from files the viewer picks.
(function () {
  "use strict";
  const FILES = {
    "retaildemo-regular.otf": ["PRetailDemo", 400, "normal"], "retaildemo-italic.otf": ["PRetailDemo", 400, "italic"],
    "retaildemo-medium.otf": ["PRetailDemo", 500, "normal"], "retaildemo-mediumitalic.otf": ["PRetailDemo", 500, "italic"],
    "retaildemo-semibold.otf": ["PRetailDemo", 600, "normal"], "retaildemo-semibolditalic.otf": ["PRetailDemo", 600, "italic"],
    "retaildemo-bold.otf": ["PRetailDemo", 700, "normal"], "retaildemo-bolditalic.otf": ["PRetailDemo", 700, "italic"],
    "valveoracle-thin.ttf": ["PValveOracle", 100, "normal"], "valveoracle-thinitalic.ttf": ["PValveOracle", 100, "italic"],
    "valveoracle-medium.ttf": ["PValveOracle", 500, "normal"], "valveoracle-mediumitalic.ttf": ["PValveOracle", 500, "italic"],
    "valveoracle-semibold.ttf": ["PValveOracle", 600, "normal"], "valveoracle-semibolditalic.ttf": ["PValveOracle", 600, "italic"],
    "valvepulp-bold.ttf": ["PValvePulp", 700, "normal"],
    "reaver-regular.otf": ["PReaver", 400, "normal"], "reaver-semibold.otf": ["PReaver", 600, "normal"], "reaver-bold.otf": ["PReaver", 700, "normal"],
    "retailtextdemo-regular.otf": ["PRetailTextDemo", 400, "normal"], "retailtextdemo-italic.otf": ["PRetailTextDemo", 400, "italic"],
    "retailtextdemo-bold.otf": ["PRetailTextDemo", 700, "normal"], "retailtextdemo-bolditalic.otf": ["PRetailTextDemo", 700, "italic"],
  };

  async function addFace(name, buffer) {
    const spec = FILES[name.toLowerCase()];
    if (!spec) return false;
    const face = new FontFace(spec[0], buffer, { weight: String(spec[1]), style: spec[2] });
    await face.load();
    document.fonts.add(face);
    return true;
  }

  // From a served folder (local calibration). Missing files are skipped.
  async function loadFromUrl(base) {
    let n = 0;
    await Promise.all(Object.keys(FILES).map(async f => {
      try {
        const r = await fetch(base + f);
        if (r.ok && await addFace(f, await r.arrayBuffer())) n++;
      } catch { /* not present */ }
    }));
    return n;
  }

  // IndexedDB cache so a viewer picks the folder once.
  function db() {
    return new Promise((res, rej) => {
      const req = indexedDB.open("broker-card-fonts", 1);
      req.onupgradeneeded = () => req.result.createObjectStore("fonts");
      req.onsuccess = () => res(req.result);
      req.onerror = () => rej(req.error);
    });
  }
  async function cacheFiles(entries) {
    try {
      const d = await db();
      const tx = d.transaction("fonts", "readwrite");
      for (const [name, buf] of entries) tx.objectStore("fonts").put(buf, name);
      await new Promise(r => { tx.oncomplete = r; tx.onerror = r; });
    } catch { /* storage blocked: fonts still apply for this visit */ }
  }
  async function loadCached() {
    try {
      const d = await db();
      const tx = d.transaction("fonts", "readonly");
      const store = tx.objectStore("fonts");
      const keys = await new Promise(r => { const q = store.getAllKeys(); q.onsuccess = () => r(q.result); q.onerror = () => r([]); });
      const vals = await new Promise(r => { const q = store.getAll(); q.onsuccess = () => r(q.result); q.onerror = () => r([]); });
      let n = 0;
      for (let i = 0; i < keys.length; i++) if (await addFace(keys[i], vals[i])) n++;
      return n;
    } catch { return 0; }
  }
  async function loadFromFiles(fileList) {
    const entries = [];
    for (const f of fileList) {
      if (!FILES[f.name.toLowerCase()]) continue;
      const buf = await f.arrayBuffer();
      if (await addFace(f.name, buf.slice(0))) entries.push([f.name.toLowerCase(), buf]);
    }
    await cacheFiles(entries);
    return entries.length;
  }

  window.PanoFonts = { FILES, loadFromUrl, loadCached, loadFromFiles, total: Object.keys(FILES).length };
})();
