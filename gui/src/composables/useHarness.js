import { ref, computed, watch, onBeforeUnmount } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';

export const DEFAULT_PORT = 9876;

function parsePort(text) {
  const s = String(text ?? '').trim();
  if (!/^\d+$/.test(s)) return null;
  const n = Number(s);
  return n >= 1 && n <= 65535 ? n : null;
}

// Per-component harness state. Nothing runs automatically: every backend call
// is triggered by an explicit user action. Generation tokens make sure a
// promise that resolves after a disconnect, port change, or unmount can never
// write into the current session.
export function useHarness() {
  // Project
  const manifestPath = ref('');
  const project = ref(null);
  const capabilities = ref(null);
  const projectLoading = ref(false); // manifest read in flight
  const pickerOpen = ref(false); // native file dialog in flight
  const projectBusy = computed(() => projectLoading.value || pickerOpen.value);
  const projectError = ref('');
  let projectGen = 0;
  let pickerGen = 0;
  let unmounted = false;

  async function pickManifest() {
    if (projectBusy.value || unmounted) return;
    const gen = ++pickerGen;
    pickerOpen.value = true;
    projectError.value = '';
    let picked;
    try {
      picked = await open({
        multiple: false,
        directory: false,
        filters: [{ name: 'Project manifest', extensions: ['json'] }],
      });
    } catch (e) {
      if (gen === pickerGen && !unmounted) projectError.value = `Picker failed: ${e}`;
      return;
    } finally {
      if (gen === pickerGen) pickerOpen.value = false;
    }
    // Closed, unmounted, or superseded while the dialog was open: drop the result.
    if (gen !== pickerGen || unmounted) return;
    if (typeof picked === 'string' && picked) {
      manifestPath.value = picked;
      await openProject();
    }
  }

  async function openProject() {
    const path = manifestPath.value.trim();
    if (!path || projectLoading.value || pickerOpen.value || unmounted) return;
    const gen = ++projectGen;
    // A different (or reopened) project must not inherit the previous Blender association.
    clearSession();
    projectLoading.value = true;
    projectError.value = '';
    try {
      const res = await invoke('harness_open_project', { path });
      if (gen !== projectGen) return;
      project.value = res.project;
      capabilities.value = res.capabilities ?? {};
    } catch (e) {
      if (gen !== projectGen) return;
      project.value = null;
      capabilities.value = null;
      projectError.value = String(e);
    } finally {
      if (gen === projectGen) projectLoading.value = false;
    }
  }

  function closeProject() {
    projectGen += 1;
    pickerGen += 1;
    project.value = null;
    capabilities.value = null;
    projectError.value = '';
    projectLoading.value = false;
    pickerOpen.value = false;
    clearSession();
  }

  // Blender scene + snapshot
  const portText = ref(String(DEFAULT_PORT));
  const portNumber = computed(() => parsePort(portText.value));
  const portValid = computed(() => portNumber.value !== null);

  const sceneBusy = ref(false);
  const sceneError = ref('');
  const scene = ref(null);
  const connectedPort = ref(null);

  const snapBusy = ref(false);
  const snapError = ref('');
  const snapshot = ref(null);
  const snapPort = ref(null);
  let sessionGen = 0;

  const now = ref(Date.now());
  let ticker = null;
  function stopTicker() {
    if (ticker) { clearInterval(ticker); ticker = null; }
  }
  watch(snapshot, (s) => {
    stopTicker();
    if (s) {
      now.value = Date.now();
      ticker = setInterval(() => { now.value = Date.now(); }, 1000);
    }
  });

  const connected = computed(() => connectedPort.value !== null && scene.value !== null);
  const canInspect = computed(() => portValid.value && !sceneBusy.value && !snapBusy.value);
  const canCapture = computed(() =>
    connected.value
    && connectedPort.value === portNumber.value
    && !sceneBusy.value
    && !snapBusy.value,
  );

  const snapshotAgeSeconds = computed(() => {
    if (!snapshot.value) return null;
    return Math.max(0, Math.round((now.value - snapshot.value.captured_at_ms) / 1000));
  });
  const snapshotAgeLabel = computed(() => {
    const s = snapshotAgeSeconds.value;
    if (s === null) return '';
    if (s < 5) return 'just now';
    if (s < 60) return `${s}s ago`;
    if (s < 3600) return `${Math.floor(s / 60)}m ${s % 60}s ago`;
    return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m ago`;
  });

  function clearSession() {
    sessionGen += 1;
    scene.value = null;
    connectedPort.value = null;
    sceneError.value = '';
    sceneBusy.value = false;
    snapshot.value = null;
    snapError.value = '';
    snapBusy.value = false;
    snapPort.value = null;
  }

  async function inspectScene() {
    const port = portNumber.value;
    if (port === null || sceneBusy.value || snapBusy.value) return;
    clearSession();
    const gen = sessionGen;
    sceneBusy.value = true;
    try {
      const res = await invoke('harness_blender_scene', { port });
      if (gen !== sessionGen) return;
      scene.value = res ?? {};
      connectedPort.value = port;
    } catch (e) {
      if (gen !== sessionGen) return;
      sceneError.value = String(e);
    } finally {
      if (gen === sessionGen) sceneBusy.value = false;
    }
  }

  async function captureSnapshot() {
    if (!canCapture.value) return;
    const port = connectedPort.value;
    const gen = sessionGen;
    snapBusy.value = true;
    snapError.value = '';
    try {
      const res = await invoke('harness_blender_snapshot', { port });
      if (gen !== sessionGen) return;
      snapshot.value = res;
      snapPort.value = port;
    } catch (e) {
      if (gen !== sessionGen) return;
      snapError.value = String(e);
    } finally {
      if (gen === sessionGen) snapBusy.value = false;
    }
  }

  // Local only: forgets the session, even mid-request. The generation bump
  // discards any pending result; the backend times out on its own. Blender keeps running.
  const canDisconnect = computed(() =>
    connected.value || !!sceneError.value || !!snapshot.value || !!snapError.value
    || sceneBusy.value || snapBusy.value,
  );
  function disconnect() {
    clearSession();
  }

  // Editing the port invalidates whatever was fetched from the old one.
  watch(portText, () => {
    if (connectedPort.value !== null || scene.value || snapshot.value || sceneError.value || sceneBusy.value || snapBusy.value) {
      clearSession();
    }
  });

  onBeforeUnmount(() => {
    unmounted = true;
    projectGen += 1;
    pickerGen += 1;
    sessionGen += 1;
    stopTicker();
  });

  return {
    manifestPath, project, capabilities, projectBusy, projectError,
    pickManifest, openProject, closeProject,
    portText, portValid, sceneBusy, sceneError, scene, connectedPort, connected,
    snapBusy, snapError, snapshot, snapPort, snapshotAgeLabel,
    canInspect, canCapture, canDisconnect,
    inspectScene, captureSnapshot, disconnect,
  };
}
