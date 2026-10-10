// Run from gui/: node --experimental-vm-modules --test tests/harness.test.mjs
//
// Loads the real useHarness.js in a vm.SourceTextModule with the Tauri imports
// replaced by controllable mocks, and drives it inside a real Vue renderer that
// writes to a plain in-memory tree (no DOM).
import { test, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import * as Vue from 'vue';

const { createRenderer, h, nextTick } = Vue;

// ---- mocks -----------------------------------------------------------------

const calls = [];
let invokeImpl;
let openImpl;

function synthetic(exportsObj) {
  const names = Object.keys(exportsObj);
  return new vm.SyntheticModule(names, function () {
    for (const n of names) this.setExport(n, exportsObj[n]);
  });
}

const sourcePath = fileURLToPath(new URL('../src/composables/useHarness.js', import.meta.url));
const mod = new vm.SourceTextModule(await readFile(sourcePath, 'utf8'), { identifier: sourcePath });
await mod.link((specifier) => {
  switch (specifier) {
    case 'vue':
      return synthetic({ ...Vue });
    case '@tauri-apps/api/core':
      return synthetic({ invoke: (cmd, args) => { calls.push([cmd, args]); return invokeImpl(cmd, args); } });
    case '@tauri-apps/plugin-dialog':
      return synthetic({ open: (opts) => openImpl(opts) });
    default:
      throw new Error(`unexpected import ${specifier}`);
  }
});
await mod.evaluate();
const { useHarness } = mod.namespace;

// ---- helpers ---------------------------------------------------------------

function deferred() {
  let resolve, reject;
  const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

const flush = async () => {
  await new Promise((r) => setImmediate(r));
  await nextTick();
};

// Minimal in-memory renderer: nodes are plain objects.
const { createApp } = createRenderer({
  patchProp() {},
  insert(el, parent, anchor) {
    const kids = parent.children ??= [];
    const i = anchor ? kids.indexOf(anchor) : -1;
    if (i >= 0) kids.splice(i, 0, el); else kids.push(el);
    el.parent = parent;
  },
  remove(el) {
    const kids = el.parent?.children;
    if (kids) kids.splice(kids.indexOf(el), 1);
  },
  createElement: (tag) => ({ tag, children: [] }),
  createText: (text) => ({ text }),
  createComment: (text) => ({ text }),
  setText(node, text) { node.text = text; },
  setElementText(node, text) { node.children = [{ text }]; },
  parentNode: (n) => n.parent ?? null,
  nextSibling(n) {
    const kids = n.parent?.children ?? [];
    return kids[kids.indexOf(n) + 1] ?? null;
  },
});

let app;
function mountHarness() {
  let harness;
  app = createApp({
    setup() {
      harness = useHarness();
      return () => h('div');
    },
  });
  app.mount({ children: [] });
  return harness;
}

beforeEach(() => {
  calls.length = 0;
  invokeImpl = async () => { throw new Error('unexpected invoke'); };
  openImpl = async () => { throw new Error('unexpected open'); };
});

afterEach(() => {
  app?.unmount(); // stops the snapshot age ticker
  app = null;
});

const cmds = () => calls.map(([c]) => c);

// ---- tests -----------------------------------------------------------------

test('scene response arriving after disconnect is ignored', async () => {
  const pending = deferred();
  invokeImpl = () => pending.promise;
  const hx = mountHarness();

  const run = hx.inspectScene();
  assert.equal(hx.sceneBusy.value, true);
  hx.disconnect();
  assert.equal(hx.sceneBusy.value, false);

  pending.resolve({ objects: [] });
  await run;
  await flush();

  assert.equal(hx.scene.value, null);
  assert.equal(hx.connected.value, false);
  assert.equal(hx.connectedPort.value, null);
});

test('changing the port invalidates scene and snapshot', async () => {
  invokeImpl = async (cmd) =>
    cmd === 'harness_blender_scene' ? { objects: [] } : { captured_at_ms: Date.now() };
  const hx = mountHarness();

  await hx.inspectScene();
  assert.equal(hx.connected.value, true);
  await hx.captureSnapshot();
  assert.ok(hx.snapshot.value);
  assert.equal(hx.snapPort.value, 9876);

  hx.portText.value = '1234';
  await flush();

  assert.equal(hx.scene.value, null);
  assert.equal(hx.snapshot.value, null);
  assert.equal(hx.snapPort.value, null);
  assert.equal(hx.connected.value, false);
  assert.equal(hx.canCapture.value, false);
});

test('changing the port mid-request discards the late scene response', async () => {
  const pending = deferred();
  invokeImpl = () => pending.promise;
  const hx = mountHarness();

  const run = hx.inspectScene();
  hx.portText.value = '1234';
  await flush();
  pending.resolve({ objects: [] });
  await run;
  await flush();

  assert.equal(hx.scene.value, null);
  assert.equal(hx.connectedPort.value, null);
  assert.equal(hx.sceneBusy.value, false);
});

test('capture is not enabled before a scene inspect', async () => {
  const hx = mountHarness();
  assert.equal(hx.canCapture.value, false);

  await hx.captureSnapshot();

  assert.deepEqual(cmds(), []);
  assert.equal(hx.snapshot.value, null);
});

test('picker resolving after unmount does not open the project', async () => {
  const picker = deferred();
  openImpl = () => picker.promise;
  const hx = mountHarness();

  const run = hx.pickManifest();
  assert.equal(hx.projectBusy.value, true);
  app.unmount();
  app = null;

  picker.resolve('/tmp/project.json');
  await run;
  await flush();

  assert.deepEqual(cmds(), []);
  assert.equal(hx.manifestPath.value, '');
  assert.equal(hx.project.value, null);
});

test('opening another project clears the previous Blender pane', async () => {
  invokeImpl = async (cmd, args) => {
    if (cmd === 'harness_open_project') return { project: { name: args.path }, capabilities: {} };
    if (cmd === 'harness_blender_scene') return { objects: [] };
    throw new Error(`unexpected ${cmd}`);
  };
  const hx = mountHarness();

  hx.manifestPath.value = '/a/project.json';
  await hx.openProject();
  await hx.inspectScene();
  assert.equal(hx.connected.value, true);
  assert.equal(hx.project.value.name, '/a/project.json');

  hx.manifestPath.value = '/b/project.json';
  await hx.openProject();

  assert.equal(hx.project.value.name, '/b/project.json');
  assert.equal(hx.scene.value, null);
  assert.equal(hx.connected.value, false);
  assert.equal(hx.connectedPort.value, null);
  assert.equal(hx.canCapture.value, false);
});
