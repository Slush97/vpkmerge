<script setup>
import { computed } from 'vue';
import { useHarness } from '../composables/useHarness.js';

const {
  manifestPath, project, capabilities, projectBusy, projectError,
  pickManifest, openProject, closeProject,
  portText, portValid, sceneBusy, sceneError, scene, connectedPort, connected,
  snapBusy, snapError, snapshot, snapPort, snapshotAgeLabel,
  canInspect, canCapture, canDisconnect,
  inspectScene, captureSnapshot, disconnect,
} = useHarness();

// Friendly rows derived from the raw capabilities. Internal flags stay in the
// technical details. File presence never implies that execution is available.
const capabilityRows = computed(() => {
  const caps = capabilities.value;
  if (!caps || typeof caps !== 'object') return [];
  const hasTool = Array.isArray(caps.tools) && caps.tools.includes('inspect_mod');
  const gamePakConfigured = !!(project.value && project.value.game_pak);
  const csdk = caps.csdk && typeof caps.csdk === 'object' ? caps.csdk : {};

  let game;
  if (!gamePakConfigured) game = { text: 'Not configured', on: false };
  else if (caps.game_pak_exists) game = { text: 'Configured, file present', on: true };
  else game = { text: 'Configured, file missing', on: false };

  let compiler;
  if (!csdk.configured) compiler = { text: 'Not configured', on: false };
  else if (csdk.compiler_exists) compiler = { text: 'Compiler found (running it is not available yet)', on: false };
  else compiler = { text: 'Configured, compiler missing', on: false };

  return [
    { key: 'vpk', label: 'VPK inspection', text: hasTool ? 'Available' : 'Not available', on: hasTool },
    { key: 'game', label: 'Game archive', ...game },
    { key: 'csdk', label: 'CSDK compiler', ...compiler },
    { key: 'model', label: 'Model assistant', text: 'Not available', on: false },
  ];
});
const capabilitiesJson = computed(() => (capabilities.value ? JSON.stringify(capabilities.value, null, 2) : ''));

const sceneJson = computed(() => (scene.value ? JSON.stringify(scene.value, null, 2) : ''));

// Short summary lines from whichever common scene fields the addon returns.
const sceneSummary = computed(() => {
  const s = scene.value;
  if (!s || typeof s !== 'object') return [];
  const rows = [];
  const scalar = (k) => (['string', 'number', 'boolean'].includes(typeof s[k]) ? String(s[k]) : null);
  for (const k of ['name', 'scene_name', 'file', 'filepath', 'blender_version', 'frame_current']) {
    const v = scalar(k);
    if (v !== null) rows.push({ key: k, text: v });
  }
  for (const k of ['objects', 'meshes', 'armatures', 'materials', 'collections']) {
    if (Array.isArray(s[k])) rows.push({ key: k, text: `${s[k].length}` });
    else if (typeof s[k] === 'number') rows.push({ key: k, text: `${s[k]}` });
  }
  return rows;
});

const sceneStatus = computed(() => {
  if (sceneBusy.value) return { text: 'Connecting to the local port and inspecting the scene...', kind: 'busy' };
  if (sceneError.value) return { text: `Could not inspect: ${sceneError.value}`, kind: 'error' };
  if (connected.value) return { text: `Scene inspected on localhost:${connectedPort.value}. Session is local to this app.`, kind: 'success' };
  return { text: 'Disconnected. Nothing is contacted until you press Inspect scene.', kind: '' };
});

const snapStatus = computed(() => {
  if (snapBusy.value) return { text: 'Capturing snapshot...', kind: 'busy' };
  if (snapError.value) {
    const earlier = snapshot.value ? ' The image below is an earlier capture, not the current Blender view.' : '';
    return { text: `Snapshot failed: ${snapError.value}${earlier}`, kind: 'error' };
  }
  if (snapshot.value) return { text: `Snapshot captured ${snapshotAgeLabel.value}`, kind: '' };
  if (connected.value) return { text: 'No snapshot yet.', kind: '' };
  return { text: 'Inspect the scene first to enable capture.', kind: '' };
});

const kindClass = (kind) => ({
  'text-ink-500 dark:text-ink-300': !kind || kind === 'busy',
  'text-green-700 dark:text-green-400': kind === 'success',
  'text-red-700 dark:text-red-400': kind === 'error',
});

const inputClass = 'bg-transparent border border-surface-300 dark:border-surface-700 rounded-md px-3 py-2 text-xs font-mono text-ink-800 dark:text-ink-100 placeholder:italic placeholder:font-serif placeholder:text-ink-500 dark:placeholder:text-ink-300 focus:outline-none focus:border-accent-500 disabled:opacity-50';
const heading = 'text-[10px] uppercase tracking-[0.18em] text-ink-500 dark:text-ink-300 font-medium';
</script>

<template>
  <div class="overflow-y-auto overflow-x-hidden min-w-0">
    <div class="p-3 sm:p-4 md:p-6 grid gap-4 md:grid-cols-[minmax(0,3fr)_minmax(0,2fr)] items-start max-w-6xl mx-auto">

      <!-- Project (main area) -->
      <section class="space-y-4 min-w-0" aria-labelledby="harness-project-title">
        <div class="paper-card rounded-md p-4">
          <h2 id="harness-project-title" class="font-serif text-xl text-ink-800 dark:text-ink-100 mb-1">Workshop</h2>
          <p class="text-xs font-serif italic text-ink-500 dark:text-ink-300 mb-3">
            Open a project manifest (JSON) to see its workspace and what this build can do with it.
          </p>
          <label :class="heading" for="harness-manifest-path" class="block mb-2">Project manifest</label>
          <div class="flex flex-wrap gap-2 items-center">
            <input
              id="harness-manifest-path"
              v-model="manifestPath"
              type="text"
              spellcheck="false"
              placeholder="/path/to/project.json"
              :disabled="projectBusy"
              :class="[inputClass, 'flex-1 min-w-[12rem]']"
              @keydown.enter="openProject"
            />
            <button type="button" class="btn" :disabled="projectBusy" @click="pickManifest">Browse</button>
            <button
              type="button"
              class="btn bg-accent-600 hover:!bg-accent-700 text-surface-0 font-medium disabled:opacity-40 disabled:cursor-not-allowed"
              :disabled="projectBusy || !manifestPath.trim()"
              @click="openProject"
            >{{ projectBusy ? 'Opening...' : 'Open' }}</button>
          </div>
          <p role="status" aria-live="polite" class="text-xs font-serif italic mt-2 min-h-[1rem]" :class="kindClass(projectError ? 'error' : '')">
            <template v-if="projectError">{{ projectError }}</template>
            <template v-else-if="projectBusy">Reading manifest...</template>
            <template v-else-if="!project">No project open.</template>
          </p>
        </div>

        <div v-if="project" class="paper-card rounded-md p-4 space-y-4">
          <div class="flex items-start justify-between gap-3">
            <div class="min-w-0">
              <h3 :class="heading">Project</h3>
              <p class="font-serif text-lg text-ink-800 dark:text-ink-100 break-words">{{ project.name }}</p>
            </div>
            <button type="button" class="btn shrink-0" @click="closeProject">Close</button>
          </div>
          <dl class="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1.5 text-xs">
            <dt class="text-ink-500 dark:text-ink-300 font-serif italic">Workspace</dt>
            <dd class="font-mono text-ink-800 dark:text-ink-100 break-all">{{ project.workspace }}</dd>
            <dt class="text-ink-500 dark:text-ink-300 font-serif italic">Schema</dt>
            <dd class="font-mono text-ink-800 dark:text-ink-100">{{ project.schema_version }}</dd>
            <template v-if="project.game_pak">
              <dt class="text-ink-500 dark:text-ink-300 font-serif italic">Game pak</dt>
              <dd class="font-mono text-ink-800 dark:text-ink-100 break-all">{{ typeof project.game_pak === 'string' ? project.game_pak : JSON.stringify(project.game_pak) }}</dd>
            </template>
          </dl>

          <div>
            <h3 :class="heading" class="mb-2">Capabilities</h3>
            <ul v-if="capabilityRows.length" class="grid gap-1.5">
              <li
                v-for="c in capabilityRows"
                :key="c.key"
                class="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-0.5 border border-surface-200 dark:border-surface-800 rounded px-2 py-1 text-xs min-w-0"
              >
                <span class="text-ink-800 dark:text-ink-100 min-w-0 break-words">{{ c.label }}</span>
                <span
                  class="font-serif italic min-w-0 break-words"
                  :class="c.on ? 'text-green-700 dark:text-green-400' : 'text-ink-500 dark:text-ink-300'"
                >{{ c.text }}</span>
              </li>
            </ul>
            <p v-else class="text-xs font-serif italic text-ink-500 dark:text-ink-300">No capabilities reported.</p>
            <details v-if="capabilitiesJson" class="mt-2">
              <summary class="text-xs font-serif italic text-accent-700 dark:text-accent-300 cursor-pointer focus-visible:outline-none focus-visible:underline">Technical details</summary>
              <pre class="mt-2 text-[11px] font-mono text-ink-800 dark:text-ink-100 bg-surface-100 dark:bg-surface-800/60 border border-surface-200 dark:border-surface-800 rounded-sm p-2 overflow-auto max-h-56 select-text">{{ capabilitiesJson }}</pre>
            </details>
          </div>

          <details v-if="project.blender || project.csdk">
            <summary class="text-xs font-serif italic text-accent-700 dark:text-accent-300 cursor-pointer focus-visible:outline-none focus-visible:underline">Tool configuration</summary>
            <pre class="mt-2 text-[11px] font-mono text-ink-800 dark:text-ink-100 bg-surface-100 dark:bg-surface-800/60 border border-surface-200 dark:border-surface-800 rounded-sm p-2 overflow-auto max-h-56 select-text">{{ JSON.stringify({ blender: project.blender, csdk: project.csdk }, null, 2) }}</pre>
          </details>
        </div>

        <!-- Future state; no runs or logs exist yet. -->
        <div class="paper-card rounded-md p-4">
          <h3 :class="heading" class="mb-1">Assistant and jobs</h3>
          <p class="text-xs font-serif italic text-ink-500 dark:text-ink-300">
            Not available in this build. GPT-driven runs and job execution are planned; no runs or logs exist yet.
          </p>
        </div>
      </section>

      <!-- Blender pane -->
      <aside class="paper-card rounded-md p-4 space-y-3 min-w-0" aria-labelledby="harness-blender-title">
        <div class="flex items-baseline justify-between gap-2">
          <h2 id="harness-blender-title" class="font-serif text-lg text-ink-800 dark:text-ink-100">Blender</h2>
          <span class="text-[10px] uppercase tracking-wide font-medium" :class="connected ? 'text-green-700 dark:text-green-400' : 'text-ink-500 dark:text-ink-300'">
            {{ connected ? 'Inspected' : 'Disconnected' }}
          </span>
        </div>
        <p class="text-[11px] font-serif italic text-ink-500 dark:text-ink-300">
          The Blender MCP addon must be running locally. This connects only to the local port you choose, and only when you press a button.
        </p>

        <div>
          <label :class="heading" for="harness-port" class="block mb-1">Local port</label>
          <div class="flex flex-wrap gap-2 items-center">
            <input
              id="harness-port"
              v-model="portText"
              type="text"
              inputmode="numeric"
              spellcheck="false"
              :disabled="sceneBusy || snapBusy"
              :aria-invalid="!portValid"
              aria-describedby="harness-port-hint"
              :class="[inputClass, 'w-24']"
              @keydown.enter="canInspect && inspectScene()"
            />
            <button
              type="button"
              class="btn bg-accent-600 hover:!bg-accent-700 text-surface-0 font-medium disabled:opacity-40 disabled:cursor-not-allowed"
              :disabled="!canInspect"
              @click="inspectScene"
            >{{ sceneBusy ? 'Inspecting...' : connected ? 'Re-inspect scene' : 'Connect and inspect' }}</button>
            <button
              type="button"
              class="btn disabled:opacity-40 disabled:cursor-not-allowed"
              :disabled="!canDisconnect"
              title="Forget this session. Blender keeps running."
              @click="disconnect"
            >Disconnect</button>
          </div>
          <p id="harness-port-hint" class="text-[11px] font-serif italic mt-1" :class="kindClass(portValid ? '' : 'error')">
            {{ portValid ? 'Port 1 to 65535. Default 9876.' : 'Enter a whole number from 1 to 65535.' }}
          </p>
        </div>

        <p role="status" aria-live="polite" class="text-xs font-serif italic" :class="kindClass(sceneStatus.kind)">
          {{ sceneStatus.text }}
        </p>

        <!-- Scene -->
        <div v-if="connected" class="space-y-2">
          <h3 :class="heading">Scene</h3>
          <dl v-if="sceneSummary.length" class="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs">
            <template v-for="r in sceneSummary" :key="r.key">
              <dt class="text-ink-500 dark:text-ink-300 font-serif italic">{{ r.key }}</dt>
              <dd class="font-mono text-ink-800 dark:text-ink-100 break-all">{{ r.text }}</dd>
            </template>
          </dl>
          <details>
            <summary class="text-xs font-serif italic text-accent-700 dark:text-accent-300 cursor-pointer focus-visible:outline-none focus-visible:underline">Raw scene JSON</summary>
            <pre class="mt-2 text-[11px] font-mono text-ink-800 dark:text-ink-100 bg-surface-100 dark:bg-surface-800/60 border border-surface-200 dark:border-surface-800 rounded-sm p-2 overflow-auto max-h-64 select-text">{{ sceneJson }}</pre>
          </details>
        </div>

        <!-- Snapshot -->
        <div class="space-y-2 border-t border-surface-200 dark:border-surface-800 pt-3">
          <div class="flex items-center justify-between gap-2">
            <h3 :class="heading">Snapshot</h3>
            <button
              type="button"
              class="btn disabled:opacity-40 disabled:cursor-not-allowed"
              :disabled="!canCapture"
              :title="canCapture ? 'Capture a still image from Blender' : 'Inspect the scene on this port first'"
              @click="captureSnapshot"
            >{{ snapBusy ? 'Capturing...' : snapshot ? 'Refresh snapshot' : 'Capture snapshot' }}</button>
          </div>
          <p role="status" aria-live="polite" class="text-xs font-serif italic" :class="kindClass(snapStatus.kind)">
            {{ snapStatus.text }}
          </p>
          <figure v-if="snapshot" class="m-0">
            <img
              :src="snapshot.data_url"
              :alt="`${snapError ? 'Earlier ' : ''}Blender snapshot from port ${snapPort}, ${snapshot.width} by ${snapshot.height}`"
              class="w-full h-auto rounded border border-surface-200 dark:border-surface-800 bg-surface-100 dark:bg-surface-900"
              :class="{ 'opacity-60': snapBusy || snapError }"
            />
            <figcaption class="mt-1 text-[11px] font-serif italic text-ink-500 dark:text-ink-300">
              <template v-if="snapError">Earlier capture, {{ snapshot.width }} x {{ snapshot.height }}, taken {{ snapshotAgeLabel }}. The latest capture failed. Not live.</template>
              <template v-else>Snapshot, {{ snapshot.width }} x {{ snapshot.height }}, captured {{ snapshotAgeLabel }}. Not live.</template>
              A snapshot does not prove game compatibility.
            </figcaption>
          </figure>
        </div>
      </aside>
    </div>
  </div>
</template>
