// Minimal Panorama style engine: parses Valve .vcss, resolves the cascade against a DOM
// built from Panorama layout tags, and translates the computed Panorama properties into
// browser CSS (flow layout -> flex/grid, opacity-brush/wash-color -> SVG multiply filters).
(function () {
  "use strict";

  const FONT_FAMILIES = {
    radiance: "PRetailDemo", "retail demo": "PRetailDemo", valveoracle: "PValveOracle", "valve oracle": "PValveOracle",
    valvepulp: "PValvePulp", "valve pulp": "PValvePulp", reaver: "PReaver", "retail text demo": "PRetailTextDemo",
    jost: "PJost",
  };
  const WEIGHTS = { thin: 100, extralight: 200, light: 300, normal: 400, regular: 400, medium: 500, "semi-bold": 600, semibold: 600, bold: 700, extrabold: 800, black: 900 };
  const NAMED = { white: [255, 255, 255, 1], black: [0, 0, 0, 1], red: [255, 0, 0, 1], yellow: [255, 255, 0, 1], gold: [255, 215, 0, 1], green: [0, 128, 0, 1], blue: [0, 0, 255, 1], transparent: [0, 0, 0, 0], none: [0, 0, 0, 0] };
  const DYNAMIC_PSEUDO = /:(hover|active|selected|disabled|enabled|focus|descendantfocus|parentdisabled|checked|lastinput|intro|outro)\b/i;
  const LABEL_TAGS = new Set(["label", "splitlabel"]);
  const INLINE_TAGS = new Set(["span", "b", "i", "strong", "em", "font"]);

  // ---------- parsing ----------
  function splitTop(s, sep) {
    const out = [];
    let depth = 0, quote = null, start = 0;
    for (let i = 0; i < s.length; i++) {
      const c = s[i];
      if (quote) { if (c === quote) quote = null; continue; }
      if (c === '"' || c === "'") quote = c;
      else if (c === "(") depth++;
      else if (c === ")") depth--;
      else if (c === sep && depth === 0) { out.push(s.slice(start, i)); start = i + 1; }
    }
    out.push(s.slice(start));
    return out;
  }

  function parseSheet(text, sheetIndex, defines, rules) {
    text = text.replace(/\/\*[\s\S]*?\*\//g, "");
    let i = 0, ruleIndex = 0;
    const n = text.length;
    while (i < n) {
      while (i < n && /\s/.test(text[i])) i++;
      if (i >= n) break;
      if (text.startsWith("@define", i)) {
        const end = text.indexOf(";", i);
        const body = text.slice(i + 7, end);
        const c = body.indexOf(":");
        defines[body.slice(0, c).trim()] = body.slice(c + 1).trim();
        i = end + 1;
        continue;
      }
      if (text.startsWith("@import", i) || text.startsWith("@charset", i)) { i = text.indexOf(";", i) + 1; continue; }
      const brace = text.indexOf("{", i);
      if (brace < 0) break;
      const selText = text.slice(i, brace).trim();
      if (selText.startsWith("@")) { // @keyframes / @media: skip balanced block
        let depth = 0, j = brace;
        for (; j < n; j++) { if (text[j] === "{") depth++; else if (text[j] === "}" && --depth === 0) break; }
        i = j + 1;
        continue;
      }
      const close = text.indexOf("}", brace);
      const body = text.slice(brace + 1, close);
      i = close + 1;
      const decls = [];
      for (const d of splitTop(body, ";")) {
        const c = d.indexOf(":");
        if (c < 0) continue;
        const prop = d.slice(0, c).trim().toLowerCase();
        let value = d.slice(c + 1).trim();
        if (!prop || !value) continue;
        const important = /!important\s*$/.test(value);
        if (important) value = value.replace(/!important\s*$/, "").trim();
        decls.push({ prop, value, important });
      }
      for (const raw of splitTop(selText, ",")) {
        const sel = raw.trim();
        if (!sel || DYNAMIC_PSEUDO.test(sel)) continue;
        try { document.createElement("div").matches(sel); } catch { continue; }
        rules.push({ sel, spec: specificity(sel), sheet: sheetIndex, order: ruleIndex, decls, key: indexKey(sel) });
      }
      ruleIndex++;
    }
  }

  function specificity(sel) {
    const s = sel.replace(/:not\(([^)]*)\)/g, " $1 ");
    const ids = (s.match(/#[\w-]+/g) || []).length;
    const cls = (s.match(/\.[\w-]+|\[[^\]]*\]|:(?!not)[\w-]+(\([^)]*\))?/g) || []).length;
    const tags = (s.replace(/#[\w-]+|\.[\w-]+|\[[^\]]*\]|:[\w-]+(\([^)]*\))?/g, " ").match(/[A-Za-z][\w-]*/g) || []).length;
    return ids * 10000 + cls * 100 + tags;
  }

  function indexKey(sel) {
    const last = sel.split(/[\s>+~]+/).filter(Boolean).pop() || "";
    const id = last.match(/#([\w-]+)/);
    if (id) return "#" + id[1];
    const cls = last.match(/\.([\w-]+)/);
    if (cls) return "." + cls[1];
    const tag = last.match(/^([A-Za-z][\w-]*)/);
    if (tag) return tag[1].toLowerCase();
    return "*";
  }

  // ---------- values ----------
  function resolveDefines(value, defines) {
    for (let pass = 0; pass < 4; pass++) {
      const next = value.replace(/url\([^)]*\)|"[^"]*"|\b([A-Za-z_][\w-]*)\b/g, (m, name) => (name && defines[name] !== undefined ? defines[name] : m));
      if (next === value) break;
      value = next;
    }
    return value;
  }

  function parseColor(str) {
    if (!str) return null;
    let s = str.trim(), alpha = null;
    const amp = s.match(/^(.*?)\s*&\s*([0-9a-fA-F]{2})$/);
    if (amp) { s = amp[1].trim(); alpha = parseInt(amp[2], 16) / 255; }
    let c = null;
    const lower = s.toLowerCase();
    if (NAMED[lower]) c = NAMED[lower].slice();
    else if (s[0] === "#") {
      const h = s.slice(1);
      if (h.length === 3) c = [...h].map(x => parseInt(x + x, 16)).concat(1);
      else if (h.length === 6 || h.length === 8) {
        c = [0, 2, 4].map(k => parseInt(h.slice(k, k + 2), 16));
        c.push(h.length === 8 ? parseInt(h.slice(6, 8), 16) / 255 : 1);
      }
    } else {
      const m = s.match(/^rgba?\(([^)]*)\)$/i);
      if (m) {
        const p = m[1].split(",").map(x => parseFloat(x));
        c = [p[0], p[1], p[2], p.length > 3 ? p[3] : 1];
      }
    }
    if (!c || c.some(v => Number.isNaN(v))) return null;
    if (alpha !== null) c[3] = alpha;
    return c;
  }
  // Source 2 composites UI in linear light. In linear mode every color and image is fed to the
  // browser linear-encoded, so its ordinary blending happens in linear space, and the root
  // converts the finished card back to sRGB.
  let LINEAR = false;
  // Blend-space exponent: 1 is true linear light; a little above 1 keeps 8-bit precision in the
  // darks at a small cost in blend accuracy.
  let K = 1;
  const toLin = v => { const c = v / 255; return Math.pow(c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4), 1 / K) * 255; };
  const cssColor = c => {
    const [r, g, b] = LINEAR ? [toLin(c[0]), toLin(c[1]), toLin(c[2])] : c;
    return `rgba(${+r.toFixed(2)},${+g.toFixed(2)},${+b.toFixed(2)},${+c[3].toFixed(4)})`;
  };

  function parseGradient(value) {
    const m = value.match(/^gradient\s*\(([\s\S]*)\)\s*$/i);
    if (!m) return null;
    const parts = splitTop(m[1], ",").map(x => x.trim());
    if (parts[0].toLowerCase() !== "linear") {
      const cols = parts.map(p => p.match(/^(?:from|to|color-stop)\s*\(([\s\S]*)\)$/i)).filter(Boolean).map(x => parseColor(splitTop(x[1], ",").pop()));
      return cols.length ? { angle: 90, stops: [[0, cols[0]], [1, cols[cols.length - 1]]] } : null;
    }
    const p1 = parts[1].split(/\s+/).map(parseFloat), p2 = parts[2].split(/\s+/).map(parseFloat);
    const angle = Math.atan2(p2[0] - p1[0], -(p2[1] - p1[1])) * 180 / Math.PI;
    const stops = [];
    for (const part of parts.slice(3)) {
      const f = part.match(/^(from|to|color-stop)\s*\(([\s\S]*)\)$/i);
      if (!f) continue;
      const args = splitTop(f[2], ",").map(x => x.trim());
      if (f[1].toLowerCase() === "from") stops.push([0, parseColor(args[0])]);
      else if (f[1].toLowerCase() === "to") stops.push([1, parseColor(args[0])]);
      else stops.push([parseFloat(args[0]) > 1 ? parseFloat(args[0]) / 100 : parseFloat(args[0]), parseColor(args[1])]);
    }
    if (stops.some(s => !s[1])) return null;
    return { angle: ((angle % 360) + 360) % 360, stops };
  }
  const cssGradient = g => `linear-gradient(${g.angle}deg, ${g.stops.map(([p, c]) => `${cssColor(c)} ${p * 100}%`).join(", ")})`;

  function fontStack(value) {
    const fams = value.split(",").map(f => f.trim().replace(/^["']|["']$/g, ""));
    const out = [];
    for (const f of fams) {
      const k = FONT_FAMILIES[f.toLowerCase()];
      if (k && !out.includes(`"${k}"`)) out.push(`"${k}"`);
    }
    out.push('"Noto Sans"', "system-ui", "sans-serif");
    return out.join(", ");
  }

  function shadowParts(value) {
    const color = parseColor(value.replace(/^([-\d.]+(px)?\s+)+/, "").trim()) || [0, 0, 0, 1];
    const nums = (value.match(/^([-\d.]+(px)?\s+)+/) || [""])[0].trim().split(/\s+/).filter(Boolean).map(parseFloat);
    const [x = 0, y = 0, blur = 0, strength = 1] = nums;
    return { x, y, blur, strength: nums.length >= 4 ? strength : 1, color };
  }

  // ---------- SVG multiply filters ----------
  const filterCache = new Map();
  let svgDefs = null;
  function ensureDefs() {
    if (svgDefs) return;
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("width", "0"); svg.setAttribute("height", "0");
    svg.style.position = "absolute";
    svgDefs = document.createElementNS("http://www.w3.org/2000/svg", "defs");
    svg.appendChild(svgDefs);
    document.body.appendChild(svg);
  }
  function linToSrgbFilter() {
    ensureDefs();
    if (!document.getElementById("pano-lin2srgb") || document.getElementById("pano-lin2srgb").dataset.k !== String(K)) {
      document.getElementById("pano-lin2srgb")?.remove();
      const f = document.createElementNS("http://www.w3.org/2000/svg", "filter");
      f.setAttribute("id", "pano-lin2srgb");
      f.dataset.k = String(K);
      f.setAttribute("color-interpolation-filters", "sRGB");
      const t = document.createElementNS("http://www.w3.org/2000/svg", "feComponentTransfer");
      const table = [];
      for (let i = 0; i <= 255; i++) { const l = Math.pow(i / 255, K); table.push((l <= 0.0031308 ? l * 12.92 : 1.055 * Math.pow(l, 1 / 2.4) - 0.055).toFixed(5)); }
      for (const ch of ["feFuncR", "feFuncG", "feFuncB"]) {
        const fn = document.createElementNS("http://www.w3.org/2000/svg", ch);
        fn.setAttribute("type", "table");
        fn.setAttribute("tableValues", table.join(" "));
        t.appendChild(fn);
      }
      f.appendChild(t);
      svgDefs.appendChild(f);
    }
    return "url(#pano-lin2srgb)";
  }
  function multiplyFilter(rgb) {
    const key = (LINEAR ? "l" : "s") + rgb.map(v => Math.round(v)).join("-");
    if (filterCache.has(key)) return filterCache.get(key);
    ensureDefs();
    const id = "pano-mul-" + key;
    const f = document.createElementNS("http://www.w3.org/2000/svg", "filter");
    f.setAttribute("id", id);
    f.setAttribute("color-interpolation-filters", "sRGB");
    const m = document.createElementNS("http://www.w3.org/2000/svg", "feColorMatrix");
    const [r, g, b] = rgb.map(v => ((LINEAR ? toLin(v) : v) / 255).toFixed(4));
    m.setAttribute("type", "matrix");
    m.setAttribute("values", `${r} 0 0 0 0 0 ${g} 0 0 0 0 0 ${b} 0 0 0 0 0 1 0`);
    f.appendChild(m);
    svgDefs.appendChild(f);
    filterCache.set(key, `url(#${id})`);
    return filterCache.get(key);
  }

  // ---------- engine ----------
  class Engine {
    constructor(sheetTexts, assets, scopes, options = {}) {
      this.linear = options.linear !== false;
      this.k = options.k || 1;
      this.uiScale = options.uiScale !== false;
      this.scopes = Object.fromEntries(Object.entries(scopes || {}).map(([k, v]) => [k, new Set(v)]));
      this.defines = {};
      this.rules = [];
      sheetTexts.forEach((t, i) => parseSheet(t, i, this.defines, this.rules));
      this.index = new Map();
      for (const r of this.rules) {
        if (!this.index.has(r.key)) this.index.set(r.key, []);
        this.index.get(r.key).push(r);
      }
      this.assets = assets || {};
      this.missing = new Set();
    }

    compute(el, allowed) {
      const cands = new Set();
      const add = k => { const l = this.index.get(k); if (l) for (const r of l) cands.add(r); };
      add("*"); add(el.tagName.toLowerCase());
      if (el.id) add("#" + el.id);
      for (const c of el.classList) add("." + c);
      const hits = [];
      for (const r of cands) if ((!allowed || allowed.has(r.sheet)) && el.matches(r.sel)) hits.push(r);
      hits.sort((a, b) => a.spec - b.spec || a.sheet - b.sheet || a.order - b.order);
      const props = {};
      const imp = {};
      for (const r of hits) for (const d of r.decls) {
        if (imp[d.prop] && !d.important) continue;
        props[d.prop] = resolveDefines(d.value, this.defines);
        if (d.important) imp[d.prop] = true;
      }
      return props;
    }

    asset(url) {
      const m = url.match(/url\(\s*["']?s2r:\/\/([^"')]+)["']?\s*\)/i) || url.match(/^s2r:\/\/(.+)$/i);
      if (!m) return null;
      const path = m[1].replace(/\.(vtex|vsvg|png|psd)$/i, "").toLowerCase();
      const a = this.assets[path];
      if (!a) { this.missing.add(path); return null; }
      return this.linear && a.lin ? { ...a, url: a.lin, srgb: a.url } : { ...a, srgb: a.url };
    }

    style(root, uiScale) {
      LINEAR = this.linear;
      K = this.k;
      root.style.zoom = uiScale;
      // Isolate the card from whatever page hosts it.
      Object.assign(root.style, { lineHeight: "normal", fontSize: "16px", fontWeight: "400", fontStyle: "normal", textTransform: "none", textAlign: "left", whiteSpace: "normal", wordSpacing: "normal", color: "#ffffff" });
      this.walk(root, { flow: "none", pad: [0, 0, 0, 0], label: false, scope: this.scopes.root || null }, root);
      this.shrinkLabels(root);
      if (this.linear) root.style.filter = [root.style.filter, linToSrgbFilter()].filter(Boolean).join(" ");
    }

    walk(el, parent, root) {
      const tag = el.tagName.toLowerCase();
      const layout = el.dataset.layout;
      const own = layout && this.scopes[layout];
      const allowed = own ? new Set([...(parent.scope || own), ...own]) : parent.scope;
      const childScope = own ? new Set(own) : parent.scope;
      const p = this.compute(el, allowed);
      const inline = parent.label && INLINE_TAGS.has(tag);
      const isLabel = LABEL_TAGS.has(tag);
      const s = el.style;
      const info = { props: p, brush: null, wash: null, opacity: 1, bgColor: null, bgGradient: null, bgImage: null };
      el.__pano = info;

      // text
      if (p.color) { const c = parseColor(p.color); if (c) s.color = cssColor(c); }
      if (p["font-family"]) s.fontFamily = fontStack(p["font-family"]);
      if (p["font-size"]) s.fontSize = p["font-size"];
      if (p["font-weight"]) s.fontWeight = WEIGHTS[p["font-weight"].toLowerCase()] || p["font-weight"];
      if (p["font-style"]) s.fontStyle = p["font-style"];
      if (p["text-transform"]) s.textTransform = p["text-transform"];
      if (p["letter-spacing"]) s.letterSpacing = p["letter-spacing"];
      if (p["line-height"]) s.lineHeight = p["line-height"];
      if (p["text-align"]) s.textAlign = p["text-align"];
      if (p["text-decoration"]) s.textDecoration = p["text-decoration"];
      if (p["white-space"]) s.whiteSpace = p["white-space"];
      if (p["text-shadow"] && p["text-shadow"] !== "none") {
        const sh = shadowParts(p["text-shadow"]);
        const col = cssColor(sh.color);
        if (sh.blur <= 0.01 && sh.x === 0 && sh.y === 0) {
          const r = Math.min(2, Math.max(0.6, sh.strength / 2.5));
          s.textShadow = [[r, 0], [-r, 0], [0, r], [0, -r], [r, r], [-r, -r], [r, -r], [-r, r]].map(([x, y]) => `${x}px ${y}px 0 ${col}`).join(",");
        } else {
          const reps = Math.max(1, Math.min(4, Math.round(sh.strength)));
          s.textShadow = Array(reps).fill(`${sh.x}px ${sh.y}px ${sh.blur}px ${col}`).join(",");
        }
      }
      if (inline) {
        if (p["background-color"]) { const c = parseColor(p["background-color"]); if (c) s.backgroundColor = cssColor(c); }
        for (const child of el.children) this.walk(child, { ...parent, scope: childScope }, root);
        return;
      }

      // layout
      const collapsed = p.visibility === "collapse";
      const flow = (p["flow-children"] || "none").toLowerCase();
      const dir = { down: "column", up: "column-reverse", right: "row", left: "row-reverse", "right-wrap": "row", "down-wrap": "column", "left-wrap": "row-reverse" }[flow];
      s.boxSizing = "border-box";
      s.position = "relative";
      s.minWidth = "0";
      if (collapsed) s.display = "none";
      else if (isLabel) { s.display = "block"; s.overflowWrap = "break-word"; }
      else if (dir) {
        s.display = "flex";
        s.flexDirection = dir;
        s.alignItems = "flex-start";
        if (flow.endsWith("wrap")) s.flexWrap = "wrap";
      } else s.display = "grid";
      if (!isLabel && !dir) { s.gridTemplateColumns = "100%"; s.gridTemplateRows = "100%"; }

      for (const k of ["margin", "margin-top", "margin-right", "margin-bottom", "margin-left", "padding", "padding-top", "padding-right", "padding-bottom", "padding-left", "min-width", "min-height", "max-width", "max-height", "border", "border-top", "border-bottom", "border-left", "border-right", "border-radius", "z-index"]) {
        if (p[k] !== undefined) s.setProperty(k, k.startsWith("border") && !k.endsWith("radius") ? this.borderCss(p[k]) : p[k]);
      }
      if (p.overflow && /clip|squish|scroll/.test(p.overflow) && !/noclip/.test(p.overflow)) s.overflow = "hidden";

      // size + placement within parent
      const pAxis = parent.flow === "none" ? null : /column/.test(parent.dir) ? "height" : "width";
      const ipf = p["ignore-parent-flow"] === "true";
      const ha = (p["horizontal-align"] || "").toLowerCase();
      const va = (p["vertical-align"] || "").toLowerCase().replace("middle", "center");
      const stackLike = ipf || parent.flow === "none";
      const marginPx = (axis) => {
        const px = x => (x && /px$/.test(x.trim()) ? parseFloat(x) : 0);
        const m = (p.margin || "").split(/\s+/).filter(Boolean).map(px);
        const [t = 0, r = t, b = t, l = r] = m;
        const a = axis === "width" ? [p["margin-left"] !== undefined ? px(p["margin-left"]) : l, p["margin-right"] !== undefined ? px(p["margin-right"]) : r]
                                   : [p["margin-top"] !== undefined ? px(p["margin-top"]) : t, p["margin-bottom"] !== undefined ? px(p["margin-bottom"]) : b];
        return a[0] + a[1];
      };
      const setSize = (axis) => {
        const v = p[axis];
        if (!v || v === "fit-children") return;
        const m = v.match(/^fill-parent-flow\(\s*([\d.]+)\s*\)$/);
        if (m) {
          if (!stackLike && axis === pAxis) { s.flexGrow = m[1]; s.flexShrink = "1"; s.flexBasis = "0px"; }
          else if (stackLike) { axis === "width" ? (s.justifySelf = "stretch") : (s.alignSelf = "stretch"); }
          else s.alignSelf = "stretch";
          return;
        }
        const hp = v.match(/^(width|height)-percentage\(\s*([\d.]+)%\s*\)$/);
        if (hp) { s.aspectRatio = axis === "width" ? `${hp[2] / 100} / 1` : `1 / ${hp[2] / 100}`; return; }
        if (v === "100%" && !ipf && (parent.flow === "none" || axis !== pAxis)) {
          if (parent.flow === "none") axis === "width" ? (s.justifySelf = "stretch") : (s.alignSelf = "stretch");
          else s.alignSelf = "stretch";
          return;
        }
        // Panorama percentages include the panel's own margins.
        const mg = marginPx(axis);
        s[axis] = /%$/.test(v) && mg ? `calc(${v} - ${mg}px)` : v;
        if (!stackLike && axis === pAxis) s.flexShrink = "0";
      };
      if (ipf) {
        s.position = "absolute";
        const [pt, pr, pb, pl] = parent.pad;
        if (p.width === "100%") { s.left = pl + "px"; s.right = pr + "px"; }
        else if (ha === "right") s.right = pr + "px";
        else if (ha === "center") { s.left = pl + "px"; s.right = pr + "px"; s.marginLeft = "auto"; s.marginRight = "auto"; s.width = "fit-content"; }
        else s.left = pl + "px";
        if (p.height === "100%") { s.top = pt + "px"; s.bottom = pb + "px"; }
        else if (va === "bottom") s.bottom = pb + "px";
        else if (va === "center") { s.top = pt + "px"; s.bottom = pb + "px"; s.marginTop = "auto"; s.marginBottom = "auto"; s.height = "fit-content"; }
        else s.top = pt + "px";
      } else if (parent.flow === "none") {
        s.gridArea = "1 / 1";
        s.justifySelf = { right: "end", center: "center" }[ha] || "start";
        s.alignSelf = { bottom: "end", center: "center" }[va] || "start";
      } else if (/column/.test(parent.dir)) {
        s.alignSelf = { right: "flex-end", center: "center" }[ha] || "flex-start";
        s.flexShrink = isLabel ? "1" : "0";
        if (va === "center" && p["margin-top"] === undefined && p["margin-bottom"] === undefined && !p.margin) { s.marginTop = "auto"; s.marginBottom = "auto"; }
        else if (va === "bottom" && p["margin-top"] === undefined && !p.margin) s.marginTop = "auto";
      } else {
        s.alignSelf = { bottom: "flex-end", center: "center" }[va] || "flex-start";
        s.flexShrink = isLabel ? "1" : "0";
        if (ha === "right" && p["margin-left"] === undefined) s.marginLeft = "auto";
      }
      if (!(ipf && p.width === "100%")) setSize("width");
      if (!(ipf && p.height === "100%")) setSize("height");

      // paint
      const filters = [];
      if (p.opacity !== undefined) { info.opacity = parseFloat(p.opacity); s.opacity = p.opacity; }
      const layers = [];
      const sizes = [], positions = [], repeats = [];
      if (p["background-image"] && p["background-image"] !== "none") {
        const a = this.asset(p["background-image"]);
        if (a) {
          info.bgImage = a;
          layers.push(`url("${a.url}")`);
          sizes.push(p["background-size"] || (tag === "image" ? "contain" : "auto"));
          positions.push(p["background-position"] || "0% 0%");
          repeats.push(p["background-repeat"] || "no-repeat");
        }
      }
      if (p["background-color"] && p["background-color"] !== "none") {
        const g = parseGradient(p["background-color"]);
        if (g) { info.bgGradient = g; layers.push(cssGradient(g)); sizes.push("100% 100%"); positions.push("0 0"); repeats.push("no-repeat"); }
        else { const c = parseColor(p["background-color"]); if (c) { info.bgColor = c; s.backgroundColor = cssColor(c); } }
      }
      if (layers.length) {
        s.backgroundImage = layers.join(", ");
        s.backgroundSize = sizes.join(", ");
        s.backgroundPosition = positions.join(", ");
        s.backgroundRepeat = repeats.join(", ");
      }
      if (tag === "image" && el.dataset.src) {
        const a = this.asset("s2r://" + el.dataset.src);
        if (a) {
          info.bgImage = a;
          s.backgroundImage = `url("${a.url}")`;
          s.backgroundSize = "contain"; s.backgroundRepeat = "no-repeat"; s.backgroundPosition = "center";
          if (!p.width) s.width = a.w + "px";
          if (!p.height) s.height = a.h + "px";
        }
      }
      if (p["opacity-mask"]) {
        const a = this.asset(p["opacity-mask"].split(/\s+(?=[\d.]+$)/)[0]);
        if (a) { s.maskImage = s.webkitMaskImage = `url("${a.url}")`; s.maskSize = s.webkitMaskSize = "100% 100%"; s.maskMode = "alpha"; }
      }
      if (p["opacity-brush"]) {
        const g = parseGradient(p["opacity-brush"]);
        if (g) {
          const c0 = g.stops[0][1], c1 = g.stops[g.stops.length - 1][1];
          info.brush = { rgb: [0, 1, 2].map(k => (c0[k] + c1[k]) / 2), a0: c0[3], a1: c1[3], angle: g.angle };
          filters.push(multiplyFilter(info.brush.rgb));
          const mask = `linear-gradient(${g.angle}deg, rgba(0,0,0,${c0[3]}), rgba(0,0,0,${c1[3]}))`;
          if (!s.maskImage) { s.maskImage = s.webkitMaskImage = mask; s.maskSize = s.webkitMaskSize = "100% 100%"; }
        }
      }
      if (p["wash-color"] && p["wash-color"] !== "none") {
        const c = parseColor(p["wash-color"]);
        if (c && !(c[0] === 255 && c[1] === 255 && c[2] === 255)) { info.wash = c; filters.push(multiplyFilter(c)); }
      }
      if (p["img-shadow"]) {
        const sh = shadowParts(p["img-shadow"]);
        const reps = Math.max(1, Math.min(3, Math.round(sh.strength)));
        for (let k = 0; k < reps; k++) filters.push(`drop-shadow(${sh.x}px ${sh.y}px ${sh.blur}px ${cssColor(sh.color)})`);
      }
      if (p["box-shadow"] && p["box-shadow"] !== "none") {
        const sh = shadowParts(p["box-shadow"].replace(/\bfill\b/, ""));
        s.boxShadow = `${sh.x}px ${sh.y}px ${sh.blur}px ${cssColor(sh.color)}`;
      }
      if (p.brightness) filters.push(`brightness(${p.brightness})`);
      if (p.saturation) filters.push(`saturate(${p.saturation})`);
      if (filters.length) s.filter = filters.join(" ");
      if (p.transform) {
        s.transform = p.transform.replace(/rotateZ\(/gi, "rotate(").replace(/translate3d\(([^,]+),([^,]+),[^)]+\)/gi, "translate($1,$2)");
      }
      if (this.uiScale && p["ui-scale"] && p["ui-scale"] !== "100%") s.zoom = parseFloat(p["ui-scale"]) / 100;

      const px = v => (v && /px$/.test(v.trim()) ? parseFloat(v) : 0);
      const pad = [0, 0, 0, 0];
      if (p.padding) { const q = p.padding.split(/\s+/).map(px); const [t, r = t, b = t, l = r] = q; pad.splice(0, 4, t, r, b, l); }
      ["padding-top", "padding-right", "padding-bottom", "padding-left"].forEach((k, i) => { if (p[k] !== undefined) pad[i] = px(p[k]); });

      const me = { flow: dir ? flow : "none", dir: dir || "", pad, label: isLabel, scope: childScope };
      for (const child of el.children) this.walk(child, me, root);
    }

    borderCss(v) {
      return v.replace(/(#[0-9a-fA-F]{3,8}|rgba?\([^)]*\)|[A-Za-z][\w]*&[0-9a-fA-F]{2})/g, m => { const c = parseColor(m); return c ? cssColor(c) : m; });
    }

    shrinkLabels(root) {
      for (const el of root.querySelectorAll("*")) {
        const p = el.__pano && el.__pano.props;
        if (!p || p["text-overflow"] !== "shrink" || el.style.display === "none") continue;
        let size = parseFloat(getComputedStyle(el).fontSize);
        let guard = 40;
        while (guard-- > 0 && size > 7 && (el.scrollWidth > el.clientWidth + 1 || el.scrollHeight > el.clientHeight + 1)) {
          size -= 0.5;
          el.style.fontSize = size + "px";
        }
      }
    }
  }

  function build(spec) {
    // spec: [tag, attrs, ...children] | string
    if (typeof spec === "string") return document.createTextNode(spec);
    const [tag, attrs = {}, ...children] = spec;
    const el = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs)) {
      if (v == null || v === false) continue;
      if (k === "class") el.className = v;
      else if (k === "html") el.innerHTML = v;
      else if (k === "text") el.textContent = v;
      else if (k === "src") el.dataset.src = v;
      else el.setAttribute(k, v);
    }
    for (const c of children) if (c) el.appendChild(build(c));
    return el;
  }

  window.Pano = { Engine, build, parseColor, parseGradient, toLin };
})();
