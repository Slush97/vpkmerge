#!/usr/bin/env node
// import-to-grimoire.mjs
// -----------------------------------------------------------------------------
// Register a locally-built VPK into the Grimoire mod manager as a tracked local
// mod, with a display name and an auto-generated annotated thumbnail that bakes
// in tracking data (test status, notes, short sha, date). Replaces a lost
// `import-local-mod.mjs`.
//
// This runs OUTSIDE Electron, so it cannot call Grimoire's IPC handlers. It
// reimplements the exact install + metadata logic of the `import-custom-mod`
// IPC handler (grimoire/electron/main/ipc/mods.ts) and the slot allocator
// (grimoire/electron/main/services/mods.ts -> allocateEnabledVpkPath) and the
// atomic metadata writer (grimoire/electron/main/services/metadata.ts).
//
// Why this exists: dropping a *_dir.vpk into citadel/addons by hand leaves it
// "unmanaged" (no mod-metadata.json entry), so Grimoire shows it with no label
// and a future scan/prune can clobber its slot. Going through this script gives
// it a metadata entry keyed by its metaKey, exactly like a real import, so it
// shows in the Installed tab with a name + notes and is NOT pruned.
//
// Usage:
//   node tools/import-to-grimoire.mjs --vpk <path_dir.vpk> --name "<Display Name>" \
//     [--notes "<free text>"] [--status untested|in-game-ok|broken] \
//     [--hero <codename>] [--nsfw] [--deadlock-path <dir>] [--dry-run]
//
// NO npm deps. Pure Node ESM. Thumbnail is an SVG data URL (see notes below).
// -----------------------------------------------------------------------------

import { createHash } from 'node:crypto';
import {
  existsSync,
  readFileSync,
  writeFileSync,
  renameSync,
  unlinkSync,
  copyFileSync,
  mkdirSync,
  readdirSync,
  statSync,
} from 'node:fs';
import { join, dirname, basename, resolve, extname } from 'node:path';
import { homedir } from 'node:os';

// ── Constants mirrored from Grimoire ────────────────────────────────────────
// grimoire/electron/main/services/mods.ts
const MIN_VPK_PRIORITY = 1;
const MAX_VPK_PRIORITY = 99;
// grimoire/electron/main/services/deadlock.ts
const MAX_ADDON_FOLDERS = 10; // base + addons1..addons9
const OVERFLOW_FOLDER_RE = /^addons(\d+)$/i;

const KNOWN_DEFAULT_DEADLOCK =
  '/home/esoc/.steam/steam/steamapps/common/Deadlock';

// Grimoire palette (grimoire/docs/design-overhaul-brief.md)
const PALETTE = {
  bg: '#0f0f0f',
  card: '#1a1a1a',
  border: '#2d2d2d',
  accent: '#f97316',
  text: '#ffffff',
  muted: '#a1a1aa',
};

// Status -> pill color. Keep the vocabulary small + obvious.
const STATUS_COLORS = {
  untested: '#a1a1aa', // muted grey
  'in-game-ok': '#22c55e', // green
  broken: '#ef4444', // red
};

// Best-effort hero codename -> Grimoire canonical display name. Only confident
// mappings (see deadlock-hero-codenames memory). Anything not here is stored as
// the raw codename in the notes context only; lockerHero is left unset so we
// never mis-file a mod in the Locker.
const HERO_CODENAME_TO_DISPLAY = {
  hornet: 'Vindicta',
  vampirebat: 'Mina',
  unicorn: 'Celeste',
  bookworm: 'Paige',
  gigawatt: 'Seven',
  necro: 'Graves',
  wraith: 'Wraith',
  inferno: 'Infernus',
  yamato: 'Yamato',
  chrono: 'Paradox',
  ghost: 'Lady Geist',
  nano: 'Calico',
  fencer: 'Apollo',
  magician: 'Sinclair',
  astro: 'Holliday',
  mcginnis: 'McGinnis',
  pocket: 'Pocket',
};

// ── CLI parsing ─────────────────────────────────────────────────────────────
function parseArgs(argv) {
  const out = {
    vpk: null,
    name: null,
    notes: null,
    status: 'untested',
    hero: null,
    nsfw: false,
    thumbnail: null,
    deadlockPath: null,
    dryRun: false,
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const next = () => {
      const v = argv[++i];
      if (v == null) throw new Error(`Missing value for ${a}`);
      return v;
    };
    switch (a) {
      case '--vpk': out.vpk = next(); break;
      case '--name': out.name = next(); break;
      case '--notes': out.notes = next(); break;
      case '--status': out.status = next(); break;
      case '--hero': out.hero = next(); break;
      case '--nsfw': out.nsfw = true; break;
      case '--thumbnail': out.thumbnail = next(); break;
      case '--deadlock-path': out.deadlockPath = next(); break;
      case '--dry-run': out.dryRun = true; break;
      case '-h':
      case '--help': out.help = true; break;
      default:
        throw new Error(`Unknown argument: ${a}`);
    }
  }
  return out;
}

const USAGE = `import-to-grimoire.mjs — register a built VPK into Grimoire as a tracked local mod

Usage:
  node tools/import-to-grimoire.mjs --vpk <path_dir.vpk> --name "<Display Name>" \\
    [--notes "<free text>"] [--status untested|in-game-ok|broken] \\
    [--hero <codename>] [--thumbnail <image>] [--nsfw] [--deadlock-path <dir>] [--dry-run]

Flags:
  --vpk            Path to the built VPK (should end in _dir.vpk for Deadlock).
  --name           Display name shown in the Installed tab. Required.
  --notes          Free text baked into the thumbnail and stored as the label.
  --status         untested (default) | in-game-ok | broken. Colors the pill.
  --hero           Hero codename (e.g. chrono). Best-effort Locker filing.
  --thumbnail      Optional SVG/PNG/JPG/WEBP/GIF image for the Grimoire card.
  --nsfw           Mark the mod NSFW.
  --deadlock-path  Override the Deadlock install dir.
  --dry-run        Print the plan + write a preview SVG to /tmp; touch nothing live.
`;

// ── Deadlock path resolution ────────────────────────────────────────────────
// app.getPath('userData') on Linux Electron resolves to ~/.config/<appName>.
// Grimoire's app name is "grimoire" (confirmed: ~/.config/grimoire/ holds
// mod-metadata.json + settings.json on this machine).
function grimoireConfigDir() {
  const xdg = process.env.XDG_CONFIG_HOME;
  const base = xdg && xdg.trim() ? xdg : join(homedir(), '.config');
  return join(base, 'grimoire');
}
function getSettingsPath() { return join(grimoireConfigDir(), 'settings.json'); }
function getMetadataPath() { return join(grimoireConfigDir(), 'mod-metadata.json'); }

// flag > settings.json (devMode-aware, mirrors getActiveDeadlockPath) > default
function resolveDeadlockPath(flag) {
  if (flag) return resolve(flag);
  const settingsPath = getSettingsPath();
  if (existsSync(settingsPath)) {
    try {
      const s = JSON.parse(readFileSync(settingsPath, 'utf-8'));
      if (s.devMode && s.devDeadlockPath) return s.devDeadlockPath;
      if (s.deadlockPath) return s.deadlockPath;
    } catch {
      // fall through to default
    }
  }
  return KNOWN_DEFAULT_DEADLOCK;
}

// ── Path helpers mirrored from deadlock.ts ──────────────────────────────────
function getCitadelPath(deadlockPath) {
  return join(deadlockPath, 'game', 'citadel');
}
function getAddonsPath(deadlockPath) {
  return join(getCitadelPath(deadlockPath), 'addons');
}
function getDisabledPath(deadlockPath) {
  return join(getAddonsPath(deadlockPath), '.disabled');
}
function overflowAddonsPath(deadlockPath, index) {
  return join(getCitadelPath(deadlockPath), `addons${index}`);
}

// Ordered addon folders: base first, then existing overflow folders numerically.
function getAddonFolderPaths(deadlockPath) {
  const citadel = getCitadelPath(deadlockPath);
  const folders = [getAddonsPath(deadlockPath)];
  try {
    const overflow = readdirSync(citadel, { withFileTypes: true })
      .filter((e) => e.isDirectory() && OVERFLOW_FOLDER_RE.test(e.name))
      .map((e) => ({ path: join(citadel, e.name), num: parseInt(e.name.match(OVERFLOW_FOLDER_RE)[1], 10) }))
      .sort((a, b) => a.num - b.num)
      .map((e) => e.path);
    folders.push(...overflow);
  } catch {
    // citadel unreadable: base-only is the safe fallback
  }
  return folders;
}

// metaKeyFor: bare filename for base/.disabled, addons{N}/<file> for overflow.
function metaKeyFor(vpkPath) {
  const fileName = basename(vpkPath);
  const parentName = basename(dirname(vpkPath));
  return OVERFLOW_FOLDER_RE.test(parentName) ? `${parentName}/${fileName}` : fileName;
}

// parseVpkPriority: pak##(_dir).vpk -> NN, else null.
function parseVpkPriority(filename) {
  if (!filename.startsWith('pak') || (!filename.endsWith('_dir.vpk') && !filename.endsWith('.vpk'))) {
    return null;
  }
  const num = parseInt(filename.slice(3, 5), 10);
  return Number.isNaN(num) ? null : num;
}

// The pakNN numbers in use in one folder.
function folderPakNumbers(folder) {
  const nums = new Set();
  if (!existsSync(folder)) return nums;
  for (const entry of readdirSync(folder)) {
    const p = parseVpkPriority(entry);
    if (p !== null) nums.add(p);
  }
  return nums;
}

function pickEnableSlot(forbidden, preferred) {
  for (const p of preferred) {
    if (p != null && Number.isInteger(p) && p >= MIN_VPK_PRIORITY && p <= MAX_VPK_PRIORITY && !forbidden.has(p)) {
      return p;
    }
  }
  for (let p = MIN_VPK_PRIORITY; p <= MAX_VPK_PRIORITY; p++) {
    if (!forbidden.has(p)) return p;
  }
  return null; // folder full
}

// allocateSlot: base first (forbidding .disabled pakNN too), then each overflow
// folder, then mint a new overflow folder. Returns { folder, fileName, minted }.
// NOTE: we do NOT actually create a new overflow folder or patch gameinfo here;
// minting an overflow folder is an Electron-side concern (fixGameinfo). If the
// allocator would need to mint one, we surface a clear error telling the user to
// import via the Grimoire GUI instead (so gameinfo gets patched correctly).
function allocateSlot(deadlockPath) {
  const disabledForbidden = folderPakNumbers(getDisabledPath(deadlockPath));
  const folders = getAddonFolderPaths(deadlockPath);
  for (let i = 0; i < folders.length; i++) {
    const folder = folders[i];
    const used = folderPakNumbers(folder);
    const forbidden = i === 0 ? new Set([...used, ...disabledForbidden]) : used;
    const slot = pickEnableSlot(forbidden, []);
    if (slot === null) continue; // full, try next folder
    return { folder, fileName: `pak${String(slot).padStart(2, '0')}_dir.vpk`, minted: false };
  }
  // All existing folders full. Grimoire would mint a fresh overflow folder and
  // patch gameinfo.gi; we can't safely patch gameinfo here, so refuse.
  const existingOverflow = folders.length - 1;
  if (existingOverflow >= MAX_ADDON_FOLDERS - 1) {
    throw new Error('Enable limit reached (990 mods). Disable one to make room.');
  }
  throw new Error(
    'All existing addon folders are full. Creating an overflow folder + patching ' +
      'gameinfo.gi must go through the Grimoire GUI import so the search path is ' +
      'registered correctly. Disable a mod or import via Grimoire instead.'
  );
}

// ── sha256 (matches setModMetadataWithHash's hashFileSha256) ────────────────
function hashFileSha256(filePath) {
  const hash = createHash('sha256');
  hash.update(readFileSync(filePath));
  return hash.digest('hex');
}

// ── Metadata load/save (mirrors metadata.ts atomic temp+rename) ─────────────
function loadMetadata() {
  const path = getMetadataPath();
  if (!existsSync(path)) return {};
  const content = readFileSync(path, 'utf-8');
  return JSON.parse(content); // throw on corrupt: never write over a bad file
}
function saveMetadata(metadata) {
  const path = getMetadataPath();
  const tempPath = `${path}.tmp`;
  const serialized = JSON.stringify(metadata, null, 2);
  // Validate it re-parses before we touch the live file.
  JSON.parse(serialized);
  try {
    writeFileSync(tempPath, serialized, 'utf-8');
    renameSync(tempPath, path);
  } catch (err) {
    try { if (existsSync(tempPath)) unlinkSync(tempPath); } catch { /* ignore */ }
    throw err;
  }
}

// ── Thumbnail generation ────────────────────────────────────────────────────
// THUMBNAIL FORMAT: SVG data URL. ModThumbnail.tsx renders src straight into an
// <img src={...}> (Chromium/Electron), which displays image/svg+xml data URLs
// fine. SVG is dependency-free (no sharp/canvas) and supports rich wrapped text,
// a colored status pill, and small print — exactly what an annotated tracking
// card needs. We base64-encode it (not utf8 percent-encoding) to match the
// existing data: thumbnails in the live metadata, which are base64.

function escapeXml(s) {
  return String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&apos;');
}

// Naive word-wrap to a max char count per line (SVG has no auto-wrap). Long
// single words are hard-split so they can't overflow the card.
function wrapText(text, maxChars, maxLines) {
  const words = String(text).split(/\s+/).filter(Boolean);
  const lines = [];
  let line = '';
  for (const word of words) {
    let w = word;
    while (w.length > maxChars) {
      if (line) { lines.push(line); line = ''; }
      lines.push(w.slice(0, maxChars));
      w = w.slice(maxChars);
    }
    const candidate = line ? `${line} ${w}` : w;
    if (candidate.length > maxChars) {
      if (line) lines.push(line);
      line = w;
    } else {
      line = candidate;
    }
    if (lines.length >= maxLines) break;
  }
  if (line && lines.length < maxLines) lines.push(line);
  if (lines.length > maxLines) lines.length = maxLines;
  // Ellipsize the last line if we truncated.
  if (words.join(' ').length > lines.join(' ').length && lines.length) {
    const last = lines[lines.length - 1];
    lines[lines.length - 1] = last.length > maxChars - 1 ? last.slice(0, maxChars - 1) + '…' : last + '…';
  }
  return lines;
}

function buildThumbnailSvg({ name, status, notes, shortSha, date, hero, nsfw }) {
  const W = 512;
  const H = 288; // 16:9, matches the aspect-video card
  const statusColor = STATUS_COLORS[status] || PALETTE.muted;
  const statusLabel = status.toUpperCase();

  const nameLines = wrapText(name, 26, 2);
  const noteLines = notes ? wrapText(notes, 46, 3) : [];

  // Layout cursors.
  let y = 56;
  const nameTspans = nameLines
    .map((ln, i) => `<text x="28" y="${y + i * 34}" font-size="30" font-weight="700" fill="${PALETTE.text}" font-family="system-ui, sans-serif">${escapeXml(ln)}</text>`)
    .join('');
  y += nameLines.length * 34 + 8;

  // Status pill + optional hero/nsfw tags.
  const pillW = 14 + statusLabel.length * 9;
  const pill = `
    <rect x="28" y="${y}" rx="11" ry="11" width="${pillW}" height="24" fill="${statusColor}" opacity="0.18"/>
    <rect x="28" y="${y}" rx="11" ry="11" width="${pillW}" height="24" fill="none" stroke="${statusColor}" stroke-width="1.5"/>
    <text x="${28 + pillW / 2}" y="${y + 16}" text-anchor="middle" font-size="13" font-weight="700" fill="${statusColor}" font-family="system-ui, sans-serif">${escapeXml(statusLabel)}</text>`;
  let tagX = 28 + pillW + 10;
  const tags = [];
  if (hero) tags.push(hero);
  if (nsfw) tags.push('NSFW');
  const tagEls = tags
    .map((t) => {
      const tw = 14 + t.length * 8;
      const el = `
        <rect x="${tagX}" y="${y}" rx="11" ry="11" width="${tw}" height="24" fill="none" stroke="${PALETTE.border}" stroke-width="1.5"/>
        <text x="${tagX + tw / 2}" y="${y + 16}" text-anchor="middle" font-size="12" font-weight="600" fill="${PALETTE.muted}" font-family="system-ui, sans-serif">${escapeXml(t)}</text>`;
      tagX += tw + 8;
      return el;
    })
    .join('');
  y += 24 + 16;

  const noteTspans = noteLines
    .map((ln, i) => `<text x="28" y="${y + i * 22}" font-size="16" fill="${PALETTE.muted}" font-family="system-ui, sans-serif">${escapeXml(ln)}</text>`)
    .join('');

  // Footer: sha + date, accent rule above it.
  const footerY = H - 26;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}">
  <rect width="${W}" height="${H}" fill="${PALETTE.bg}"/>
  <rect x="6" y="6" width="${W - 12}" height="${H - 12}" rx="14" fill="${PALETTE.card}" stroke="${PALETTE.border}" stroke-width="1.5"/>
  <rect x="6" y="6" width="6" height="${H - 12}" rx="3" fill="${PALETTE.accent}"/>
  <text x="28" y="32" font-size="13" font-weight="700" letter-spacing="2" fill="${PALETTE.accent}" font-family="system-ui, sans-serif">GRIMOIRE LOCAL</text>
  ${nameTspans}
  ${pill}
  ${tagEls}
  ${noteTspans}
  <line x1="28" y1="${footerY - 18}" x2="${W - 28}" y2="${footerY - 18}" stroke="${PALETTE.border}" stroke-width="1"/>
  <text x="28" y="${footerY}" font-size="14" fill="${PALETTE.muted}" font-family="ui-monospace, monospace">sha ${escapeXml(shortSha)}</text>
  <text x="${W - 28}" y="${footerY}" text-anchor="end" font-size="14" fill="${PALETTE.muted}" font-family="system-ui, sans-serif">${escapeXml(date)}</text>
</svg>`;
  return svg;
}

function svgToDataUrl(svg) {
  const b64 = Buffer.from(svg, 'utf-8').toString('base64');
  return `data:image/svg+xml;base64,${b64}`;
}

function imageMimeForPath(filePath) {
  switch (extname(filePath).toLowerCase()) {
    case '.svg': return 'image/svg+xml';
    case '.png': return 'image/png';
    case '.jpg':
    case '.jpeg': return 'image/jpeg';
    case '.webp': return 'image/webp';
    case '.gif': return 'image/gif';
    default:
      throw new Error('Unsupported thumbnail type. Use SVG, PNG, JPG, WEBP, or GIF.');
  }
}

function readImageDataUrl(imagePath) {
  const path = resolve(imagePath);
  if (!existsSync(path)) throw new Error(`Thumbnail not found: ${path}`);
  if (!statSync(path).isFile()) throw new Error(`Thumbnail is not a file: ${path}`);

  const mime = imageMimeForPath(path);
  const bytes = readFileSync(path);
  return {
    dataUrl: `data:${mime};base64,${bytes.toString('base64')}`,
    mime,
    path,
    size: bytes.length,
  };
}

// ── Main ────────────────────────────────────────────────────────────────────
function main() {
  let args;
  try {
    args = parseArgs(process.argv.slice(2));
  } catch (e) {
    console.error(`Error: ${e.message}\n`);
    process.stdout.write(USAGE);
    process.exit(2);
  }
  if (args.help) {
    process.stdout.write(USAGE);
    return;
  }

  // Validate required args.
  if (!args.vpk) fail('--vpk is required.');
  if (!args.name || !args.name.trim()) fail('--name is required.');
  if (!STATUS_COLORS[args.status]) {
    fail(`--status must be one of: ${Object.keys(STATUS_COLORS).join(', ')}`);
  }

  const vpkPath = resolve(args.vpk);
  if (!existsSync(vpkPath)) fail(`VPK not found: ${vpkPath}`);
  if (!vpkPath.toLowerCase().endsWith('.vpk')) fail(`Not a .vpk: ${vpkPath}`);
  if (!vpkPath.toLowerCase().endsWith('_dir.vpk')) {
    console.warn(`Warning: ${basename(vpkPath)} does not end in _dir.vpk. ` +
      'Deadlock requires the _dir.vpk chunk-directory name to load it in-game.');
  }

  // Resolve + validate Deadlock path.
  const deadlockPath = resolveDeadlockPath(args.deadlockPath);
  const addonsPath = getAddonsPath(deadlockPath);
  if (!existsSync(addonsPath)) {
    fail(`Deadlock addons dir not found: ${addonsPath}\n` +
      `(resolved Deadlock path: ${deadlockPath}). Pass --deadlock-path to override.`);
  }

  // Metadata file must exist + parse (we never write over a corrupt one).
  const metadataPath = getMetadataPath();
  let metadata;
  if (existsSync(metadataPath)) {
    try {
      metadata = loadMetadata();
    } catch (e) {
      fail(`mod-metadata.json is not valid JSON, refusing to touch it: ${e.message}`);
    }
  } else {
    metadata = {}; // first import: a fresh map is fine
    console.warn(`Note: ${metadataPath} does not exist yet; will create it.`);
  }

  const name = args.name.trim();

  // Idempotent re-import: if an existing entry already carries this exact
  // modName AND is flagged as a local import from this tool, reuse its slot
  // (overwrite the file + bump notes) instead of allocating a fresh slot.
  let destPath = null;
  let destMetaKey = null;
  let reused = false;
  for (const [key, val] of Object.entries(metadata)) {
    if (val && val.localImport && val.modName === name) {
      const folder = findFolderForMetaKey(deadlockPath, key);
      if (folder) {
        destPath = join(folder, basename(key));
        destMetaKey = key;
        reused = true;
        break;
      }
    }
  }

  // Otherwise allocate the next free slot the same way Grimoire does.
  if (!destPath) {
    const slot = allocateSlot(deadlockPath);
    destPath = join(slot.folder, slot.fileName);
    destMetaKey = metaKeyFor(destPath);
  }

  // Hash the SOURCE vpk; after copy the dest is byte-identical so the hash is
  // the same as setModMetadataWithHash(destPath) would compute.
  const sha256 = hashFileSha256(vpkPath);
  const shortSha = sha256.slice(0, 8);
  const date = new Date().toISOString().slice(0, 10);

  // Hero filing (best-effort, never mis-file).
  const heroDisplay = args.hero ? HERO_CODENAME_TO_DISPLAY[args.hero.toLowerCase()] : null;

  // Build the annotated thumbnail.
  const svg = buildThumbnailSvg({
    name,
    status: args.status,
    notes: args.notes || '',
    shortSha,
    date,
    hero: heroDisplay || args.hero || null,
    nsfw: args.nsfw,
  });
  let thumbnailUrl = svgToDataUrl(svg);
  let thumbnailSummary = `generated SVG data URL, ${thumbnailUrl.length} chars (raw svg ${svg.length} bytes)`;
  let previewSvg = svg;
  if (args.thumbnail) {
    let image;
    try {
      image = readImageDataUrl(args.thumbnail);
    } catch (e) {
      fail(e.message);
    }
    thumbnailUrl = image.dataUrl;
    thumbnailSummary = `${image.mime} data URL from ${image.path}, ${thumbnailUrl.length} chars (${image.size} bytes)`;
    previewSvg = image.mime === 'image/svg+xml' ? readFileSync(image.path, 'utf-8') : null;
  }

  // Build the metadata entry. We mirror the import-custom-mod IPC's core fields
  // (modName, thumbnailUrl, nsfw, sha256) and add:
  //   - localImport: marker so this tool can find + idempotently reuse the slot,
  //     and so it's clearly distinguishable from a GameBanana download.
  //   - sourceSection: "LocalImport" — a clear non-GameBanana provenance string.
  //   - fileDescription: the notes, which Grimoire surfaces as the variant label.
  //   - lockerHero / lockerHeroSource: only when we have a confident mapping.
  //   - localImportStatus / localImportDate / localImportHero: tracking sidecar.
  // We deliberately do NOT set gameBananaId/categoryId so enrichMod treats it as
  // a local mod. globalType/abilitySounds are left for enrichMod to classify.
  const entry = {
    modName: name,
    thumbnailUrl,
    nsfw: !!args.nsfw,
    sha256,
    localImport: true,
    sourceSection: 'LocalImport',
    localImportStatus: args.status,
    localImportDate: date,
  };
  if (args.notes && args.notes.trim()) entry.fileDescription = args.notes.trim();
  if (args.hero) entry.localImportHero = args.hero;
  if (heroDisplay) {
    entry.lockerHero = heroDisplay;
    entry.lockerHeroSource = 'manual';
  }

  // ── Dry run: print plan + write preview, touch nothing live ───────────────
  if (args.dryRun) {
    const previewPath = '/tmp/grimoire-import-preview.svg';
    if (previewSvg) writeFileSync(previewPath, previewSvg, 'utf-8');
    console.log('── DRY RUN (no live changes) ─────────────────────────────');
    console.log(`Deadlock path:   ${deadlockPath}`);
    console.log(`Metadata file:   ${metadataPath}`);
    console.log(`Source VPK:      ${vpkPath}`);
    console.log(`Chosen slot:     ${reused ? 'REUSED existing local-import slot' : 'next free slot'}`);
    console.log(`Dest path:       ${destPath}`);
    console.log(`metaKey:         ${destMetaKey}`);
    console.log(`sha256:          ${sha256}`);
    console.log(`Thumbnail:       ${thumbnailSummary}`);
    if (previewSvg) console.log(`Preview written: ${previewPath}`);
    console.log('\nMetadata entry it WOULD write (keyed by metaKey):');
    console.log(JSON.stringify({ [destMetaKey]: redactThumb(entry) }, null, 2));
    console.log('\n(thumbnailUrl shown truncated above; full data URL is written on a real run.)');
    return;
  }

  // ── Live import ───────────────────────────────────────────────────────────
  // 1. Copy the VPK to the dest slot.
  mkdirSync(dirname(destPath), { recursive: true });
  copyFileSync(vpkPath, destPath);

  // 2. Scrub any orphan metadata at this slot, then write the fresh entry.
  //    (Mirrors removeModMetadata + setModMetadataWithHash in the IPC handler.)
  delete metadata[destMetaKey];
  metadata[destMetaKey] = entry;
  saveMetadata(metadata);

  // 3. Re-read + validate the live file parses after the write.
  try {
    const reread = JSON.parse(readFileSync(metadataPath, 'utf-8'));
    if (!reread[destMetaKey]) throw new Error('entry missing after write');
  } catch (e) {
    fail(`Post-write validation failed (metadata may be inconsistent): ${e.message}`);
  }

  console.log('── Imported ──────────────────────────────────────────────');
  console.log(`Name:        ${name}`);
  console.log(`Status:      ${args.status}`);
  console.log(`Installed:   ${destPath}`);
  console.log(`metaKey:     ${destMetaKey}`);
  console.log(`sha256:      ${sha256}`);
  if (reused) console.log('(reused an existing local-import slot — overwrote in place)');
  console.log('\nRefresh the Installed tab in Grimoire, and restart Deadlock to load it.');
}

// Find which addon folder currently holds a given metaKey, if any (used for
// idempotent re-import).
function findFolderForMetaKey(deadlockPath, metaKey) {
  const fileName = basename(metaKey);
  for (const folder of getAddonFolderPaths(deadlockPath)) {
    if (existsSync(join(folder, fileName)) && metaKeyFor(join(folder, fileName)) === metaKey) {
      return folder;
    }
  }
  // Slot recorded in metadata but file gone: still reuse the recorded location.
  if (metaKey.includes('/')) {
    const [folderName] = metaKey.split('/');
    return join(getCitadelPath(deadlockPath), folderName);
  }
  return getAddonsPath(deadlockPath);
}

function redactThumb(entry) {
  const e = { ...entry };
  if (typeof e.thumbnailUrl === 'string' && e.thumbnailUrl.length > 64) {
    e.thumbnailUrl = `${e.thumbnailUrl.slice(0, 48)}…[${e.thumbnailUrl.length} chars]`;
  }
  return e;
}

function fail(msg) {
  console.error(`Error: ${msg}`);
  process.exit(1);
}

main();
