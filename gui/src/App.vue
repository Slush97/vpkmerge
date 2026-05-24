<script setup>
import { ref, computed, onMounted, onBeforeUnmount, watch, nextTick } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useSettings } from './composables/useSettings.js';
import { useLogs } from './composables/useLogs.js';
import BrowseTab from './components/BrowseTab.vue';

const { settings, setTheme, setDoodleTheme, setCandleEnabled, THEMES, DOODLE_THEMES } = useSettings();
const { log: logInfo, warn: logWarn, error: logError, formatExport: formatLogsExport } = useLogs();
const showSettingsModal = ref(false);
const settingsModalRef = ref(null);
const DOODLE_LABELS = {
  arcane: 'Arcane',
  celestial: 'Celestial',
  botanical: 'Botanical',
  nautical: 'Nautical',
};
const THEME_LABELS = {
  light: 'Light',
  dark: 'Dark',
  system: 'System',
};

const mods = ref([]);
const outputPath = ref('');
const outputExists = ref(false);
const status = ref({ text: '', kind: '' });
const busy = ref(false);
const isDragging = ref(false);
const showConflictsModal = ref(false);
const showMergedModal = ref(false);
const lastReport = ref(null);
const lastFocused = ref(null);
const conflictsModalRef = ref(null);
const mergedModalRef = ref(null);
const policy = ref('first_wins');
const overrides = ref(new Map());

// Top-level tab. 'merge' is the original view; 'browse' is the VPK file
// browser (auto-loads the local Deadlock install).
const activeTab = ref('merge');
const TABS = [
  { key: 'merge', label: 'Merge' },
  { key: 'browse', label: 'Browse' },
];

// Texture preview cache. Keyed by `${vpkPath}::${entry}`. Values are one of:
//   { state: 'loading' }
//   { state: 'ok', dataUrl, width, height, origWidth, origHeight, format }
//   { state: 'error', message }
const texturePreviews = ref(new Map());

function isPreviewablePath(path) {
  return typeof path === 'string' && path.toLowerCase().endsWith('.vtex_c');
}

function previewKey(vpkPath, entry) {
  return `${vpkPath}::${entry}`;
}

function getPreview(vpkPath, entry) {
  return texturePreviews.value.get(previewKey(vpkPath, entry));
}

async function ensurePreview(vpkPath, entry) {
  const key = previewKey(vpkPath, entry);
  if (texturePreviews.value.has(key)) return;
  // Mark as loading immediately so the row shows a skeleton.
  const next = new Map(texturePreviews.value);
  next.set(key, { state: 'loading' });
  texturePreviews.value = next;
  try {
    const p = await invoke('preview_texture', {
      vpkPath,
      entry,
      maxDim: 96,
    });
    const ok = new Map(texturePreviews.value);
    ok.set(key, {
      state: 'ok',
      dataUrl: p.data_url,
      width: p.width,
      height: p.height,
      origWidth: p.orig_width,
      origHeight: p.orig_height,
      format: p.format,
    });
    texturePreviews.value = ok;
  } catch (e) {
    const err = new Map(texturePreviews.value);
    err.set(key, { state: 'error', message: String(e) });
    texturePreviews.value = err;
  }
}

// Kick off previews for every previewable conflict when the modal opens.
// Fires fire-and-forget; results stream into the cache as they land.
function prefetchPreviewsForConflicts() {
  for (const c of conflicts.value) {
    if (!isPreviewablePath(c.path)) continue;
    for (const idx of c.owners) {
      const mod = mods.value[idx];
      if (mod) ensurePreview(mod.path, c.path);
    }
  }
}

const POLICY_LABELS = {
  first_wins: 'Top wins',
  last_wins: 'Bottom wins',
  strict: 'Refuse',
};

function formatBytes(n) {
  if (n < 1024) return `${n} B`;
  const units = ['KB', 'MB', 'GB'];
  let v = n / 1024;
  for (const u of units) {
    if (v < 1024) return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${u}`;
    v /= 1024;
  }
  return `${Math.round(v)} TB`;
}

const conflicts = computed(() => {
  const owners = new Map();
  mods.value.forEach((mod, idx) => {
    for (const p of mod.file_paths) {
      if (!owners.has(p)) owners.set(p, []);
      owners.get(p).push(idx);
    }
  });
  const out = [];
  for (const [path, idxs] of owners) {
    if (idxs.length > 1) {
      out.push({ path, winner: idxs[0], owners: idxs });
    }
  }
  return out.sort((a, b) => a.path.localeCompare(b.path));
});

const conflictsByMod = computed(() => {
  const counts = new Map();
  for (const c of conflicts.value) {
    for (const idx of c.owners) {
      counts.set(idx, (counts.get(idx) || 0) + 1);
    }
  }
  return counts;
});

const customizedCount = computed(() => {
  let n = 0;
  for (const c of conflicts.value) if (overrides.value.has(c.path)) n += 1;
  return n;
});

const unresolvedStrict = computed(() => {
  if (policy.value !== 'strict') return 0;
  let n = 0;
  for (const c of conflicts.value) if (!overrides.value.has(c.path)) n += 1;
  return n;
});

const canMerge = computed(() =>
  mods.value.length >= 2 && !busy.value && unresolvedStrict.value === 0,
);

const mergeBlockedReason = computed(() => {
  if (mods.value.length < 2) return '';
  if (unresolvedStrict.value > 0) {
    return `Refuse policy: pick a winner for ${unresolvedStrict.value} conflict${unresolvedStrict.value === 1 ? '' : 's'}.`;
  }
  return '';
});

const mergeSummary = computed(() => {
  if (mods.value.length === 0) return '';
  const parts = [`${mods.value.length} mod${mods.value.length === 1 ? '' : 's'}`];
  if (conflicts.value.length) {
    parts.push(`${conflicts.value.length} conflict${conflicts.value.length === 1 ? '' : 's'}`);
  }
  parts.push(POLICY_LABELS[policy.value].toLowerCase());
  return parts.join(' · ');
});

function setStatus(text, kind = '') {
  status.value = { text, kind };
}

function defaultOutputPath(firstInputPath) {
  if (!firstInputPath) return '';
  const sepIdx = Math.max(firstInputPath.lastIndexOf('/'), firstInputPath.lastIndexOf('\\'));
  if (sepIdx < 0) return 'combined_dir.vpk';
  const dir = firstInputPath.substring(0, sepIdx);
  const sep = firstInputPath.charAt(sepIdx);
  return `${dir}${sep}combined_dir.vpk`;
}

async function loadPaths(paths) {
  const vpks = paths.filter((p) => p.toLowerCase().endsWith('.vpk'));
  if (vpks.length === 0) {
    setStatus('No .vpk files in that drop', 'error');
    logWarn(`drop ignored: no .vpk in ${paths.length} path(s)`);
    return;
  }
  for (const path of vpks) {
    if (mods.value.some((m) => m.path === path)) continue;
    try {
      const mod = await invoke('add_mod', { path });
      mods.value.push(mod);
      logInfo(`added mod: ${mod.name} (${mod.file_count} files, ${formatBytes(mod.size_bytes || 0)})`);
    } catch (e) {
      setStatus(`Failed to load ${path}: ${e}`, 'error');
      logError(`failed to load ${path}: ${e}`);
    }
  }
  if (mods.value.length > 0 && !outputPath.value) {
    outputPath.value = defaultOutputPath(mods.value[0].path);
  }
}

async function pickViaDialog() {
  setStatus('');
  let paths;
  try {
    paths = await invoke('pick_vpk_files');
  } catch (e) {
    setStatus(`Picker failed: ${e}`, 'error');
    return;
  }
  if (!paths?.length) return;
  await loadPaths(paths);
}

async function browseOutput() {
  setStatus('');
  try {
    const path = await invoke('pick_output_path');
    if (path) outputPath.value = path;
  } catch (e) {
    setStatus(`Picker failed: ${e}`, 'error');
  }
}

async function doMerge() {
  if (!outputPath.value) {
    await browseOutput();
    if (!outputPath.value) return;
  }
  busy.value = true;
  setStatus('Merging...');
  logInfo(`merge started: ${mods.value.length} inputs, policy=${policy.value}, overrides=${overrides.value.size}`);
  try {
    const overridesObj = {};
    for (const [path, idx] of overrides.value) overridesObj[path] = idx;
    const report = await invoke('merge_vpks', {
      orderedPaths: mods.value.map((m) => m.path),
      outputPath: outputPath.value,
      policy: policy.value,
      overrides: overridesObj,
    });
    lastReport.value = report;
    showMergedModal.value = true;
    setStatus(`Wrote ${report.total_entries} entries`, 'success');
    logInfo(`merge completed: ${report.total_entries} entries to ${report.output_path}`);
  } catch (e) {
    setStatus(`Merge failed: ${e}`, 'error');
    logError(`merge failed: ${e}`);
  } finally {
    busy.value = false;
  }
}

async function exportLogs() {
  try {
    const stamp = new Date().toISOString().replace(/[:.]/g, '-');
    const savedTo = await invoke('save_text_file', {
      defaultName: `vpkmerge-${stamp}.log`,
      content: formatLogsExport(),
    });
    if (savedTo) {
      setStatus(`Logs saved to ${savedTo}`, 'success');
      logInfo(`logs exported to ${savedTo}`);
    }
  } catch (e) {
    setStatus(`Could not save logs: ${e}`, 'error');
    logError(`logs export failed: ${e}`);
  }
}

function setOverride(path, idx) {
  const next = new Map(overrides.value);
  next.set(path, idx);
  overrides.value = next;
}

function resetOverrides() {
  overrides.value = new Map();
}

function clearOverride(path) {
  if (!overrides.value.has(path)) return;
  const next = new Map(overrides.value);
  next.delete(path);
  overrides.value = next;
}

function effectiveWinner(c) {
  if (overrides.value.has(c.path)) return overrides.value.get(c.path);
  if (policy.value === 'first_wins') return c.owners[0];
  if (policy.value === 'last_wins') return c.owners[c.owners.length - 1];
  return null;
}

function removeMod(idx) {
  mods.value.splice(idx, 1);
  if (mods.value.length === 0) outputPath.value = '';
}

function clearAll() {
  mods.value = [];
  outputPath.value = '';
  overrides.value = new Map();
  setStatus('');
}

watch(
  () => mods.value.map((m) => m.path).join('\x1f'),
  () => { if (overrides.value.size) overrides.value = new Map(); },
);

async function revealOutput() {
  if (!lastReport.value?.output_path) return;
  try {
    await invoke('reveal_in_folder', { path: lastReport.value.output_path });
  } catch (e) {
    setStatus(`Could not open folder: ${e}`, 'error');
  }
}

const dragSrcIdx = ref(null);
const dragOverIdx = ref(null);
function onDragStart(idx, e) {
  dragSrcIdx.value = idx;
  e.dataTransfer.effectAllowed = 'move';
}
function onDragOver(idx, e) {
  e.preventDefault();
  e.dataTransfer.dropEffect = 'move';
  if (dragOverIdx.value !== idx) dragOverIdx.value = idx;
}
function onDrop(idx) {
  if (dragSrcIdx.value !== null && dragSrcIdx.value !== idx) {
    const [moved] = mods.value.splice(dragSrcIdx.value, 1);
    mods.value.splice(idx, 0, moved);
  }
  dragSrcIdx.value = null;
  dragOverIdx.value = null;
}
function onDragEnd() {
  dragSrcIdx.value = null;
  dragOverIdx.value = null;
}

const isMaximized = ref(false);
async function refreshMaximized() {
  try { isMaximized.value = await getCurrentWindow().isMaximized(); } catch { /* noop */ }
}
async function winMinimize() { await getCurrentWindow().minimize(); }
async function winToggleMax() {
  const w = getCurrentWindow();
  if (await w.isMaximized()) await w.unmaximize();
  else await w.maximize();
  await refreshMaximized();
}
async function winClose() { await getCurrentWindow().close(); }

let outputCheckTimer = null;
watch(outputPath, (p) => {
  if (outputCheckTimer) clearTimeout(outputCheckTimer);
  if (!p) { outputExists.value = false; return; }
  outputCheckTimer = setTimeout(async () => {
    try { outputExists.value = await invoke('path_exists', { path: p }); }
    catch { outputExists.value = false; }
  }, 200);
});

watch(() => mods.value.length, async (n) => {
  const title = n === 0 ? 'vpkmerge' : `vpkmerge - ${n} mod${n === 1 ? '' : 's'} loaded`;
  try { await getCurrentWindow().setTitle(title); } catch { /* noop */ }
});

function onWindowKeydown(e) {
  if (e.key !== 'Escape') return;
  if (showSettingsModal.value) { showSettingsModal.value = false; e.stopPropagation(); }
  else if (showMergedModal.value) { showMergedModal.value = false; e.stopPropagation(); }
  else if (showConflictsModal.value) { showConflictsModal.value = false; e.stopPropagation(); }
}

watch([showConflictsModal, showMergedModal, showSettingsModal], async ([cv, mv, sv], [pcv, pmv, psv]) => {
  const anyOpen = cv || mv || sv;
  const wasOpen = pcv || pmv || psv;
  if (anyOpen && !wasOpen) {
    lastFocused.value = document.activeElement;
    window.addEventListener('keydown', onWindowKeydown);
    await nextTick();
    const target = sv ? settingsModalRef.value : (mv ? mergedModalRef.value : conflictsModalRef.value);
    target?.focus?.();
    if (cv) prefetchPreviewsForConflicts();
  } else if (!anyOpen && wasOpen) {
    window.removeEventListener('keydown', onWindowKeydown);
    const prior = lastFocused.value;
    lastFocused.value = null;
    if (prior && typeof prior.focus === 'function') prior.focus();
  }
});

let unlistenResize = null;
let unlistenDragDrop = null;
onMounted(async () => {
  logInfo(`vpkmerge 0.2.0 session start; theme=${settings.theme}, doodle=${settings.doodleTheme}, candle=${settings.candleEnabled}`);
  await refreshMaximized();
  unlistenResize = await getCurrentWindow().onResized(() => refreshMaximized());
  unlistenDragDrop = await getCurrentWebview().onDragDropEvent((event) => {
    if (event.payload.type === 'enter' || event.payload.type === 'over') {
      isDragging.value = true;
    } else if (event.payload.type === 'leave') {
      isDragging.value = false;
    } else if (event.payload.type === 'drop') {
      isDragging.value = false;
      loadPaths(event.payload.paths || []);
    }
  });
});
onBeforeUnmount(() => {
  if (unlistenDragDrop) unlistenDragDrop();
  if (unlistenResize) unlistenResize();
  window.removeEventListener('keydown', onWindowKeydown);
  if (outputCheckTimer) clearTimeout(outputCheckTimer);
});
</script>

<template>
  <div class="w-screen h-screen flex flex-col select-none cursor-default bg-surface-0 dark:bg-surface-950 text-ink-800 dark:text-ink-100">
    <div class="relative flex flex-col w-full h-full bg-paper overflow-hidden">

      <!-- Custom title bar -->
      <div
        data-tauri-drag-region
        class="select-none flex items-center justify-between h-9 px-3 text-ink-700 dark:text-ink-100 relative z-10"
      >
        <div data-tauri-drag-region class="flex items-center gap-2 text-xs font-serif tracking-wide pointer-events-none">
          <div class="w-2 h-2 rounded-full bg-accent-600 dark:bg-accent-300" />
          <span>vpkmerge</span>
        </div>
        <div class="flex items-center gap-1">
          <button
            type="button"
            class="w-7 h-7 rounded-md inline-flex items-center justify-center hover:bg-accent-700/10 dark:hover:bg-accent-300/10 transition active:scale-95 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent-700/60 dark:focus-visible:ring-accent-300/60 focus-visible:ring-inset"
            @click="showSettingsModal = true"
            aria-label="Settings"
            title="Settings"
          >
            <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <circle cx="8" cy="8" r="2.2"/>
              <path d="M8 1.5 L9 3 L11 2.6 L11.6 4.4 L13.4 5 L13 7 L14.5 8 L13 9 L13.4 11 L11.6 11.6 L11 13.4 L9 13 L8 14.5 L7 13 L5 13.4 L4.4 11.6 L2.6 11 L3 9 L1.5 8 L3 7 L2.6 5 L4.4 4.4 L5 2.6 L7 3 Z"/>
            </svg>
          </button>
          <button
            type="button"
            class="w-7 h-7 rounded-md inline-flex items-center justify-center hover:bg-accent-700/10 dark:hover:bg-accent-300/10 transition active:scale-95 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent-700/60 dark:focus-visible:ring-accent-300/60 focus-visible:ring-inset"
            @click="winMinimize"
            aria-label="Minimize"
          >
            <svg width="12" height="12" viewBox="0 0 12 12" fill="currentColor"><rect x="2" y="5.5" width="8" height="1" /></svg>
          </button>
          <button
            type="button"
            class="w-7 h-7 rounded-md inline-flex items-center justify-center hover:bg-accent-700/10 dark:hover:bg-accent-300/10 transition active:scale-95 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent-700/60 dark:focus-visible:ring-accent-300/60 focus-visible:ring-inset"
            @click="winToggleMax"
            :aria-label="isMaximized ? 'Restore' : 'Maximize'"
          >
            <svg v-if="!isMaximized" width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1">
              <rect x="2.5" y="2.5" width="7" height="7" />
            </svg>
            <svg v-else width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1">
              <rect x="3.5" y="3.5" width="6" height="6" />
              <path d="M5 3.5 V2 H10 V7 H8.5" />
            </svg>
          </button>
          <button
            type="button"
            class="w-7 h-7 rounded-md inline-flex items-center justify-center hover:bg-red-700 hover:text-ink-50 transition active:scale-95 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-red-600 focus-visible:ring-inset"
            @click="winClose"
            aria-label="Close"
          >
            <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.2">
              <path d="M2.5 2.5 L9.5 9.5 M9.5 2.5 L2.5 9.5" />
            </svg>
          </button>
        </div>
      </div>

      <!-- Tab strip: top-level routes inside the app shell. -->
      <div
        role="tablist"
        aria-label="Sections"
        class="flex items-center gap-1 px-3 pt-1 pb-0 border-b border-surface-200 dark:border-surface-800 relative z-10"
      >
        <button
          v-for="t in TABS"
          :key="t.key"
          type="button"
          role="tab"
          :aria-selected="activeTab === t.key"
          @click="activeTab = t.key"
          class="text-xs font-serif tracking-wide px-3 py-1.5 rounded-t-md transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent-700/45 dark:focus-visible:ring-accent-300/45"
          :class="activeTab === t.key
            ? 'text-accent-700 dark:text-accent-300 bg-surface-100/70 dark:bg-surface-800/40 border-b-2 border-accent-600 dark:border-accent-300 -mb-px'
            : 'text-ink-500 dark:text-ink-300 hover:text-ink-800 dark:hover:text-ink-100 border-b-2 border-transparent -mb-px'"
        >{{ t.label }}</button>
      </div>

      <!-- Content + footer share a single doodle overlay so the whole sheet
           below the title bar carries the same tiled doodles. -->
      <div class="doodle-overlay flex-1 min-h-0 flex flex-col">
      <div v-if="activeTab === 'merge'" class="flex-1 min-h-0 overflow-y-auto">
        <div class="min-h-full flex flex-col p-3 sm:p-4 md:p-8">

          <!-- Empty state -->
          <div v-if="mods.length === 0" class="flex-1 w-full flex items-center justify-center px-2 py-4">
            <button
              type="button"
              @click="pickViaDialog"
              class="empty-state paper-card paper-card-pressable w-full max-w-xl flex flex-col items-center justify-center gap-y-3 py-10 px-6 rounded-lg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-700/45 dark:focus-visible:ring-accent-300/45"
              :class="{ '!border-accent-300 !bg-accent-300/5': isDragging }"
            >
              <div class="relative aspect-square w-[clamp(5rem,24vh,12rem)] flex items-center justify-center pointer-events-none">
                <svg
                  viewBox="-110 -110 220 220"
                  class="absolute inset-0 w-full h-full overflow-visible text-accent-700/45 dark:text-accent-300/40"
                  aria-hidden="true"
                >
                  <circle cx="0" cy="0" r="92" fill="none" stroke="currentColor" stroke-width="1" stroke-dasharray="2.4 4.8" />
                  <circle cx="0" cy="0" r="76" fill="none" stroke="currentColor" stroke-width="0.6" />
                </svg>
                <span class="text-5xl text-accent-700 dark:text-accent-300 font-serif leading-none">⤓</span>
              </div>
              <h3 class="font-serif text-xl sm:text-2xl tracking-wide text-ink-800 dark:text-ink-100 text-center">
                {{ isDragging ? 'Drop to add' : 'Ready to merge' }}
              </h3>
              <p class="text-xs sm:text-sm font-serif italic text-ink-500 dark:text-ink-300 text-center">
                Drop VPK files here or click to browse
              </p>
            </button>
          </div>

          <!-- Non-empty: add control, mod list, output -->
          <div v-else class="w-full max-w-2xl mx-auto space-y-5">

            <button
              type="button"
              @click="pickViaDialog"
              class="paper-card paper-card-pressable w-full py-3 px-4 rounded-md flex items-center justify-center gap-2 text-sm font-serif italic border-accent-700/30 dark:border-accent-300/30 text-accent-700 dark:text-accent-300 hover:!border-accent-700 dark:hover:!border-accent-300"
              :class="{ '!border-accent-300 !bg-accent-300/10': isDragging }"
            >
              <span class="text-base leading-none">+</span>
              {{ isDragging ? 'Drop to add more' : 'Add more VPKs' }}
            </button>

            <div class="flex items-center justify-between px-1">
              <span class="text-[10px] uppercase tracking-[0.18em] text-ink-500 dark:text-ink-300 font-medium">
                {{ mods.length }} {{ mods.length === 1 ? 'mod' : 'mods' }}
              </span>
              <button
                type="button"
                @click="clearAll"
                class="text-xs italic font-serif text-ink-500 dark:text-ink-300 hover:text-red-600 dark:hover:text-red-400 focus-visible:outline-none focus-visible:underline rounded"
              >Clear all</button>
            </div>

            <div class="paper-card rounded-md p-1">
              <ul class="flex flex-col">
                <li
                  v-for="(mod, idx) in mods"
                  :key="mod.path"
                  draggable="true"
                  class="group flex items-center gap-3 px-3 py-2.5 rounded-sm cursor-grab select-none transition-colors hover:bg-surface-100/60 dark:hover:bg-surface-800/60 border-t-2 border-transparent"
                  :class="{
                    'opacity-40': dragSrcIdx === idx,
                    '!border-accent-500 dark:!border-accent-300': dragOverIdx === idx && dragSrcIdx !== null && dragSrcIdx !== idx,
                  }"
                  @dragstart="onDragStart(idx, $event)"
                  @dragover="onDragOver(idx, $event)"
                  @drop="onDrop(idx)"
                  @dragend="onDragEnd"
                >
                  <span class="text-ink-500 dark:text-ink-300 text-lg leading-none opacity-50 group-hover:opacity-100">≡</span>
                  <span class="text-ink-500 dark:text-ink-300 text-xs tabular-nums w-5 text-right font-mono">{{ idx + 1 }}</span>
                  <span class="flex-1 font-serif text-base text-ink-800 dark:text-ink-100 truncate" :title="mod.path">
                    {{ mod.name }}
                  </span>
                  <span
                    v-if="conflictsByMod.get(idx)"
                    class="text-[10px] tracking-wide px-1.5 py-0.5 rounded-full bg-accent-600/15 dark:bg-accent-300/15 text-accent-700 dark:text-accent-300 font-medium tabular-nums whitespace-nowrap"
                    :title="`${conflictsByMod.get(idx)} path${conflictsByMod.get(idx) === 1 ? '' : 's'} conflict with another mod`"
                  >{{ conflictsByMod.get(idx) }} conflict{{ conflictsByMod.get(idx) === 1 ? '' : 's' }}</span>
                  <span class="text-ink-500 dark:text-ink-300 text-xs tabular-nums italic font-serif whitespace-nowrap">{{ mod.file_count }} files</span>
                  <span class="text-ink-500 dark:text-ink-300 text-xs tabular-nums italic font-serif whitespace-nowrap">{{ formatBytes(mod.size_bytes || 0) }}</span>
                  <button
                    class="text-ink-500 dark:text-ink-300 hover:text-red-600 dark:hover:text-red-400 text-lg leading-none px-2 py-0.5 rounded opacity-50 group-hover:opacity-100 focus-visible:outline-none focus-visible:opacity-100 focus-visible:ring-1 focus-visible:ring-accent-700/45 dark:focus-visible:ring-accent-300/45"
                    title="Remove"
                    @click.stop="removeMod(idx)"
                  >×</button>
                </li>
              </ul>
            </div>

            <p class="text-xs font-serif italic text-ink-500 dark:text-ink-300 text-center">
              Drag to reorder. Mods higher in the list win on conflict.
            </p>

            <!-- Output -->
            <div class="paper-card rounded-md p-4">
              <h4 class="text-[10px] uppercase tracking-[0.18em] text-ink-500 dark:text-ink-300 font-medium mb-2">
                Output
              </h4>
              <div class="flex gap-2 items-center">
                <input
                  type="text"
                  v-model="outputPath"
                  spellcheck="false"
                  placeholder="Auto-set from the first VPK added..."
                  class="flex-1 bg-transparent border border-surface-300 dark:border-surface-700 rounded-md px-3 py-2 text-xs font-mono text-ink-800 dark:text-ink-100 placeholder:italic placeholder:font-serif placeholder:text-ink-500 dark:placeholder:text-ink-300 focus:outline-none focus:border-accent-500"
                />
                <button class="btn" @click="browseOutput">Browse</button>
              </div>
              <p
                v-if="outputExists"
                class="text-xs font-serif italic text-accent-700 dark:text-accent-300 mt-2"
              >Will overwrite the existing file at this path.</p>
            </div>

            <!-- On conflict -->
            <div class="paper-card rounded-md p-4">
              <div class="flex items-baseline justify-between mb-2">
                <h4 class="text-[10px] uppercase tracking-[0.18em] text-ink-500 dark:text-ink-300 font-medium">
                  On conflict
                </h4>
                <span class="text-[10px] italic font-serif text-ink-500 dark:text-ink-300">
                  Default: higher mod in the list wins
                </span>
              </div>
              <div role="radiogroup" aria-label="Collision policy" class="flex gap-1 p-1 bg-surface-100/70 dark:bg-surface-800/40 rounded-md">
                <button
                  v-for="key in ['first_wins', 'last_wins', 'strict']"
                  :key="key"
                  type="button"
                  role="radio"
                  :aria-checked="policy === key"
                  @click="policy = key"
                  class="flex-1 text-xs font-medium py-1.5 px-2 rounded transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent-700/45 dark:focus-visible:ring-accent-300/45"
                  :class="policy === key
                    ? 'bg-accent-600 text-surface-0'
                    : 'text-ink-700 dark:text-ink-300 hover:bg-surface-200/60 dark:hover:bg-surface-700/40'"
                >{{ POLICY_LABELS[key] }}</button>
              </div>
              <p class="text-[11px] font-serif italic text-ink-500 dark:text-ink-300 mt-2">
                <span v-if="policy === 'first_wins'">Higher mods in the list override lower ones on collision.</span>
                <span v-else-if="policy === 'last_wins'">Lower mods in the list win; higher duplicates are dropped.</span>
                <span v-else>Refuse to merge if any path collides. Resolve manually via "view conflicts".</span>
              </p>
            </div>

            <!-- Merge -->
            <div class="paper-card rounded-md p-4">
              <div class="flex items-baseline justify-between mb-3">
                <h4 class="text-[10px] uppercase tracking-[0.18em] text-ink-500 dark:text-ink-300 font-medium">
                  Merge
                </h4>
                <span class="text-[10px] italic font-serif text-ink-500 dark:text-ink-300 truncate ml-2">
                  {{ mergeSummary }}
                </span>
              </div>

              <div
                class="flex items-center gap-2 mb-3 min-h-[1.25rem]"
                aria-live="polite"
              >
                <svg
                  v-if="busy"
                  class="shrink-0 w-4 h-4 animate-spin text-accent-700 dark:text-accent-300"
                  viewBox="0 0 16 16"
                  fill="none"
                  aria-hidden="true"
                >
                  <circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="1.5" stroke-opacity="0.25" />
                  <path d="M14 8a6 6 0 0 0-6-6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
                </svg>
                <span
                  class="text-xs font-serif italic truncate flex-1"
                  :class="{
                    'text-ink-500 dark:text-ink-300': !status.kind,
                    'text-green-700 dark:text-green-400 not-italic font-sans': status.kind === 'success',
                    'text-red-700 dark:text-red-400 not-italic font-sans': status.kind === 'error',
                  }"
                >
                  {{ status.text || (mods.length >= 2 ? 'Ready when you are' : mods.length === 1 ? 'Add one more VPK to merge' : 'Drop a VPK to start') }}
                </span>
                <button
                  v-if="conflicts.length"
                  @click="showConflictsModal = true"
                  class="text-xs flex items-center gap-1.5 shrink-0 text-accent-700 dark:text-accent-300 hover:underline italic font-serif focus-visible:outline-none focus-visible:underline rounded"
                >
                  <span class="bg-accent-600 text-surface-0 font-bold rounded-full px-2 py-0.5 tracking-wider text-[10px] not-italic font-sans">{{ conflicts.length }}</span>
                  view conflicts
                </button>
              </div>

              <button
                :disabled="!canMerge"
                :title="mergeBlockedReason || 'Merge the listed VPKs'"
                class="merge-button w-full h-12 rounded-md font-medium text-base bg-accent-600 hover:!bg-accent-700 text-surface-0 disabled:opacity-40 disabled:cursor-not-allowed transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-300 focus-visible:ring-offset-2 focus-visible:ring-offset-surface-0 dark:focus-visible:ring-offset-surface-950"
                @click="doMerge"
              >Merge VPKs</button>

              <p
                v-if="mergeBlockedReason"
                class="text-[11px] font-serif italic text-red-700 dark:text-red-400 mt-2"
              >
                {{ mergeBlockedReason }}
              </p>
            </div>
          </div>
        </div>
      </div>
      <BrowseTab v-else-if="activeTab === 'browse'" class="flex-1 min-h-0" />
      </div>

      <!-- Warm vignette: a soft candle-light glow. Toggled by html[data-candle="on"]. -->
      <div class="candle-glow" aria-hidden="true" />
    </div>

    <!-- Conflicts modal -->
    <Transition name="fx-rise">
      <div
        v-if="showConflictsModal"
        class="fixed inset-0 z-50 bg-black/50 dark:bg-black/70 flex items-center justify-center p-6"
        @click.self="showConflictsModal = false"
      >
        <div
          ref="conflictsModalRef"
          tabindex="-1"
          role="dialog"
          aria-modal="true"
          aria-labelledby="conflicts-modal-title"
          class="paper-card rounded-md w-full max-w-2xl max-h-[80vh] flex flex-col focus:outline-none"
        >
          <header class="flex items-center justify-between px-5 py-4 border-b border-surface-200 dark:border-surface-800">
            <div>
              <h4 class="text-[10px] uppercase tracking-[0.18em] text-ink-500 dark:text-ink-300 font-medium">
                Path conflicts
              </h4>
              <h2 id="conflicts-modal-title" class="font-serif text-xl sm:text-2xl text-ink-800 dark:text-ink-100">
                {{ conflicts.length }} {{ conflicts.length === 1 ? 'collision' : 'collisions' }}<span
                  v-if="customizedCount"
                  class="text-sm sm:text-base text-accent-700 dark:text-accent-300 font-normal italic"
                > ({{ customizedCount }} customized)</span>
              </h2>
            </div>
            <button
              class="text-ink-500 dark:text-ink-300 hover:text-ink-800 dark:hover:text-ink-100 text-2xl leading-none px-2"
              @click="showConflictsModal = false"
            >×</button>
          </header>
          <div class="overflow-y-auto p-5 space-y-3 flex-1">
            <p class="text-xs font-serif italic text-ink-500 dark:text-ink-300 -mt-1 mb-1">
              Click a row to make it the winner for that path. Reordering mods clears overrides.
            </p>
            <div
              v-for="c in conflicts"
              :key="c.path"
              class="border rounded-md px-3 py-2.5"
              :class="overrides.has(c.path)
                ? 'border-accent-500/60 bg-accent-500/5'
                : 'border-surface-200 dark:border-surface-800'"
            >
              <div class="flex items-center gap-2 mb-1.5">
                <div class="font-mono text-xs text-ink-800 dark:text-ink-100 break-all flex-1">{{ c.path }}</div>
                <span
                  v-if="overrides.has(c.path)"
                  class="text-[10px] uppercase tracking-wide px-1.5 py-0.5 rounded-full bg-accent-600 text-surface-0 font-medium shrink-0"
                >edited</span>
                <span
                  v-else-if="effectiveWinner(c) === null"
                  class="text-[10px] uppercase tracking-wide px-1.5 py-0.5 rounded-full bg-red-700 text-surface-0 font-medium shrink-0"
                  title="Refuse policy: pick a winner or change the policy"
                >no winner</span>
              </div>
              <div class="flex flex-col gap-0.5">
                <button
                  v-for="idx in c.owners"
                  :key="idx"
                  type="button"
                  @click="setOverride(c.path, idx)"
                  class="text-xs flex items-center gap-2 font-serif text-left px-1.5 py-1 rounded transition-colors hover:bg-surface-100/80 dark:hover:bg-surface-800/80 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent-700/45 dark:focus-visible:ring-accent-300/45"
                  :class="idx === effectiveWinner(c)
                    ? 'text-accent-700 dark:text-accent-300 font-semibold'
                    : 'text-ink-500 dark:text-ink-300 line-through italic'"
                  :aria-pressed="idx === effectiveWinner(c)"
                >
                  <span v-if="idx === effectiveWinner(c)" class="text-accent-700 dark:text-accent-300">✓</span>
                  <span v-else class="text-ink-500 dark:text-ink-300">·</span>
                  <template v-if="isPreviewablePath(c.path)">
                    <span
                      class="inline-flex items-center justify-center w-12 h-12 shrink-0 rounded border border-surface-200 dark:border-surface-800 bg-surface-100/60 dark:bg-surface-900/60 overflow-hidden text-[10px] text-ink-500 dark:text-ink-300"
                      :title="getPreview(mods[idx].path, c.path)?.state === 'error'
                        ? getPreview(mods[idx].path, c.path).message
                        : (getPreview(mods[idx].path, c.path)?.format || '')"
                    >
                      <img
                        v-if="getPreview(mods[idx].path, c.path)?.state === 'ok'"
                        :src="getPreview(mods[idx].path, c.path).dataUrl"
                        :alt="`${mods[idx].name} preview`"
                        class="w-full h-full object-contain"
                        style="image-rendering: pixelated;"
                      />
                      <span
                        v-else-if="getPreview(mods[idx].path, c.path)?.state === 'error'"
                        class="not-italic"
                      >?</span>
                      <span
                        v-else
                        class="animate-pulse not-italic"
                        aria-label="loading preview"
                      >…</span>
                    </span>
                  </template>
                  {{ mods[idx].name }}
                  <span class="text-ink-500 dark:text-ink-300 not-italic ml-auto text-[10px] uppercase tracking-wide">
                    {{ idx === effectiveWinner(c) ? 'wins' : 'click to pick' }}
                  </span>
                </button>
              </div>
              <button
                v-if="overrides.has(c.path)"
                type="button"
                @click.stop="clearOverride(c.path)"
                class="text-[10px] italic font-serif text-ink-500 dark:text-ink-300 hover:text-accent-700 dark:hover:text-accent-300 mt-1.5 focus-visible:outline-none focus-visible:underline rounded"
              >Reset to default</button>
            </div>
          </div>
          <footer class="px-5 py-3 border-t border-surface-200 dark:border-surface-800 flex items-center justify-between">
            <button
              type="button"
              @click="resetOverrides"
              :disabled="!customizedCount"
              class="text-xs italic font-serif text-ink-500 dark:text-ink-300 hover:text-accent-700 dark:hover:text-accent-300 disabled:opacity-30 disabled:cursor-not-allowed focus-visible:outline-none focus-visible:underline rounded"
            >Reset all overrides</button>
            <button class="btn" @click="showConflictsModal = false">Done</button>
          </footer>
        </div>
      </div>
    </Transition>

    <!-- Merged success modal -->
    <Transition name="fx-rise">
      <div
        v-if="showMergedModal && lastReport"
        class="fixed inset-0 z-50 bg-black/50 dark:bg-black/70 flex items-center justify-center p-6"
        @click.self="showMergedModal = false"
      >
        <div
          ref="mergedModalRef"
          tabindex="-1"
          role="dialog"
          aria-modal="true"
          aria-labelledby="merged-modal-title"
          class="paper-card rounded-md w-full max-w-md flex flex-col focus:outline-none"
        >
          <header class="px-5 py-4 border-b border-surface-200 dark:border-surface-800 flex items-center gap-3">
            <span class="check-badge shrink-0" aria-hidden="true">
              <svg viewBox="0 0 32 32" width="32" height="32" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
                <circle class="check-ring" cx="16" cy="16" r="13"/>
                <path class="check-tick" d="M9 16.5 L14 21 L23 11"/>
              </svg>
            </span>
            <div>
              <h4 class="text-[10px] uppercase tracking-[0.18em] text-ink-500 dark:text-ink-300 font-medium">
                Merged
              </h4>
              <h2 id="merged-modal-title" class="font-serif text-xl sm:text-2xl text-ink-800 dark:text-ink-100">
                {{ lastReport.total_entries }} {{ lastReport.total_entries === 1 ? 'entry' : 'entries' }}
              </h2>
            </div>
          </header>

          <div class="px-5 py-4 space-y-3">
            <p class="text-xs font-serif italic text-ink-500 dark:text-ink-300">
              Wrote to
            </p>
            <code class="block font-mono text-xs text-ink-800 dark:text-ink-100 bg-surface-100 dark:bg-surface-800/60 rounded-sm px-2 py-1.5 break-all border border-surface-200 dark:border-surface-800">
              {{ lastReport.output_path }}
            </code>
            <p class="text-xs font-serif italic text-ink-500 dark:text-ink-300">
              From {{ lastReport.inputs }} {{ lastReport.inputs === 1 ? 'input' : 'inputs' }}<span v-if="lastReport.overridden">, {{ lastReport.overridden }} {{ lastReport.overridden === 1 ? 'path' : 'paths' }} overridden</span>.
            </p>
          </div>

          <footer class="px-5 py-4 border-t border-surface-200 dark:border-surface-800 flex items-center justify-end gap-2">
            <button class="btn" @click="showMergedModal = false">Close</button>
            <button
              class="btn bg-accent-600 hover:!bg-accent-700 text-surface-0 font-medium"
              @click="revealOutput"
            >Open folder</button>
          </footer>
        </div>
      </div>
    </Transition>

    <!-- Settings modal -->
    <Transition name="fx-rise">
      <div
        v-if="showSettingsModal"
        class="fixed inset-0 z-50 bg-black/50 dark:bg-black/70 flex items-center justify-center p-6"
        @click.self="showSettingsModal = false"
      >
        <div
          ref="settingsModalRef"
          tabindex="-1"
          role="dialog"
          aria-modal="true"
          aria-labelledby="settings-modal-title"
          class="paper-card rounded-md w-full max-w-md flex flex-col focus:outline-none"
        >
          <header class="flex items-center justify-between px-5 py-4 border-b border-surface-200 dark:border-surface-800">
            <h2 id="settings-modal-title" class="font-serif text-xl sm:text-2xl text-ink-800 dark:text-ink-100">
              Settings
            </h2>
            <button
              class="text-ink-500 dark:text-ink-300 hover:text-ink-800 dark:hover:text-ink-100 text-2xl leading-none px-2"
              @click="showSettingsModal = false"
              aria-label="Close"
            >×</button>
          </header>

          <div class="px-5 py-4 space-y-5">
            <!-- Theme -->
            <div>
              <h4 class="text-[10px] uppercase tracking-[0.18em] text-ink-500 dark:text-ink-300 font-medium mb-2">
                Theme
              </h4>
              <div role="radiogroup" aria-label="Theme" class="flex gap-1 p-1 bg-surface-100/70 dark:bg-surface-800/40 rounded-md">
                <button
                  v-for="key in THEMES"
                  :key="key"
                  type="button"
                  role="radio"
                  :aria-checked="settings.theme === key"
                  @click="setTheme(key)"
                  class="flex-1 text-xs font-medium py-1.5 px-2 rounded transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent-700/45 dark:focus-visible:ring-accent-300/45"
                  :class="settings.theme === key
                    ? 'bg-accent-600 text-surface-0'
                    : 'text-ink-700 dark:text-ink-300 hover:bg-surface-200/60 dark:hover:bg-surface-700/40'"
                >{{ THEME_LABELS[key] }}</button>
              </div>
            </div>

            <!-- Doodle theme -->
            <div>
              <h4 class="text-[10px] uppercase tracking-[0.18em] text-ink-500 dark:text-ink-300 font-medium mb-2">
                Doodles
              </h4>
              <div role="radiogroup" aria-label="Doodle theme" class="grid grid-cols-2 gap-1 p-1 bg-surface-100/70 dark:bg-surface-800/40 rounded-md">
                <button
                  v-for="key in DOODLE_THEMES"
                  :key="key"
                  type="button"
                  role="radio"
                  :aria-checked="settings.doodleTheme === key"
                  @click="setDoodleTheme(key)"
                  class="text-xs font-medium py-1.5 px-2 rounded transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent-700/45 dark:focus-visible:ring-accent-300/45"
                  :class="settings.doodleTheme === key
                    ? 'bg-accent-600 text-surface-0'
                    : 'text-ink-700 dark:text-ink-300 hover:bg-surface-200/60 dark:hover:bg-surface-700/40'"
                >{{ DOODLE_LABELS[key] }}</button>
              </div>
            </div>

            <div>
              <h4 class="text-[10px] uppercase tracking-[0.18em] text-ink-500 dark:text-ink-300 font-medium mb-2">
                Candlelight
              </h4>
              <label class="flex items-center justify-between gap-3 cursor-pointer select-none">
                <span class="text-sm font-serif text-ink-800 dark:text-ink-100">
                  Warm glow in the corner
                </span>
                <input
                  type="checkbox"
                  class="checkbox"
                  :checked="settings.candleEnabled"
                  @change="setCandleEnabled($event.target.checked)"
                />
              </label>
            </div>

            <div>
              <h4 class="text-[10px] uppercase tracking-[0.18em] text-ink-500 dark:text-ink-300 font-medium mb-2">
                Diagnostics
              </h4>
              <div class="flex items-center justify-between gap-3">
                <span class="text-sm font-serif text-ink-800 dark:text-ink-100">
                  Export session log
                </span>
                <button class="btn" @click="exportLogs">Export</button>
              </div>
            </div>
          </div>

          <footer class="px-5 py-3 border-t border-surface-200 dark:border-surface-800 flex items-center justify-end">
            <button class="btn" @click="showSettingsModal = false">Done</button>
          </footer>
        </div>
      </div>
    </Transition>
  </div>
</template>
