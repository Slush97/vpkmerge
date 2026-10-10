# Modding harness scaffold

Status: runnable local CLI and Rust library, plus a Workshop tab in the existing
desktop app. The tab opens projects, inspects a local Blender scene, and captures
an on-demand viewport snapshot. No model requests, MCP transport, live video,
Blender scene editing, or CSDK execution yet.

See the [architecture and delivery contract](../docs/modding-harness-design.md)
for the Blender side pane, process boundaries, durable jobs, and milestone gates.

## Stack decision

Use Rust for project state, tool contracts, VPK operations, and future job execution.
Use the existing Tauri 2 + Vue 3 frontend for the eventual workbench. Keep the
library independent of Tauri so CLI, MCP, and desktop callers share behavior.

Tauri uses the system webview; Electron embeds Chromium and Node. Electron is an
alternative if a measured rendering requirement needs Chromium, but introduces
another backend bridge here. GTK4/libadwaita is an alternative for an explicitly
GNOME-focused product; it would require a new UI implementation.

Sources: [Tauri architecture](https://v2.tauri.app/concept/architecture/),
[Electron introduction](https://www.electronjs.org/docs/latest/),
[libadwaita](https://gnome.pages.gitlab.gnome.org/libadwaita/).

## Try it

Run from the repository root:

```bash
cargo run -p vpkmerge-harness -- init .scratch/harness-demo --name "Mod workshop"
cargo run -p vpkmerge-harness -- doctor .scratch/harness-demo/mod-project.json
```

`init` refuses to overwrite an existing manifest. Edit the generated JSON to set
absolute paths for `game_pak` and optional integration settings:

```json
{
  "blender": {
    "mcp_server_name": "blender",
    "blend_file": null
  },
  "csdk": {
    "root": "/path/to/Reduced_CSDK_12",
    "proton": "/path/to/proton"
  }
}
```

Those are fields to insert into the full manifest, not a replacement manifest.
The doctor checks file presence only; it does not establish compiler compatibility
or Blender connectivity. The Blender server name refers to your agent host's MCP
configuration, not the addon's TCP socket.

## Desktop Workshop

Run `pnpm dev` from `gui/`, then select Workshop. Open a project manifest created
by the CLI. With the Blender MCP addon running in Blender, select its local port
(default 9876), choose Connect and inspect, review the scene, then Capture snapshot.
Captures are still images with an age label. Disconnect forgets the local view;
it does not stop Blender. Changing tabs requires reconnecting.

This first adapter uses the installed addon's JSON-over-TCP protocol directly,
restricted to IPv4 loopback. It is not an MCP client and does not use the manifest's
MCP server name. It supports only scene inspection and screenshot requests, with
bounded timeouts and response sizes. Screenshot paths are private temporary files
chosen by the application; files are validated and removed after reading.

Scene inspection establishes what the addon reported at that moment, not a durable
Blender session identity. File changes inside Blender are not yet tracked. Live
streaming, project-linked session identities, and revision checks remain later work.
Automated protocol tests use fake local servers; a real Blender/window smoke test
is still required to validate the installed addon's viewport context and appearance.

Call tools with one JSON request on stdin:

```bash
printf '%s\n' '{"tool":"inspect_mod","arguments":{"path":"/path/to/mod_dir.vpk","offset":0,"limit":50}}' |
  cargo run -p vpkmerge-harness -- call .scratch/harness-demo/mod-project.json
```

Available calls: `get_capabilities` (empty arguments), `inspect_mod` (`path`,
`offset`, `limit`), `detect_conflicts` (`inputs`, `offset`, `limit`). Lists are
sorted and paginated, with limits of 1 to 500. Conflict owner indices refer to
the supplied input order. Relative tool input paths resolve against the caller's
working directory; use absolute paths for portable calls.

Successful calls return `{ "ok": true, "result": ... }`. Tool/runtime failures
return `{ "ok": false, "error": ... }` and exit 1. CLI usage errors use clap's
normal stderr diagnostics. This single-call protocol is not MCP or an OpenAI
function schema. The Rust `ToolCall` and `execute` function are the shared seam
for those future transports. The local CLI has ordinary user filesystem access;
it is not a sandbox or a remotely exposed service.

## Skills and workflow evidence

The repository-owned [skills](skills/) are source assets for the harness, not
automatically installed global skills. An agent can read the relevant `SKILL.md`
now; a future host adapter should discover their frontmatter and load on demand.
References assume this repository layout is retained.

Use [the existing MCP proposal](../docs/mcp-server-design.md) as prior design,
and [the modding knowledge base](../docs/deadlock-modding/README.md) as evidence.
Some older Blender docs describe preview-only GLB export; current model-edit and
CSDK routes must be checked individually before promising an authoring round trip.

## Next implementation slices

1. Staged merge/build jobs with explicit collision policy, source fingerprints,
   entry-level validation, durable run records, and cancellation.
2. MCP server transport for these tools; host-managed Blender MCP discovery and
   calls. Save a `.blend` checkpoint and record object/skeleton/material mappings.
3. CSDK adapter around existing compiler wrappers: unique addon staging, logs,
   timeouts, output freshness checks, and tool/game build provenance.
4. Model-provider tool loop, skill loading, and real-job evaluations. Store API
   credentials outside project manifests; do not hard-code a model choice.
5. Tauri workbench for projects, runs, previews, and artifacts. Run blocking asset
   work outside the UI thread. Add managed installation when requested.

Keep inventory inspection, offline validation, and retail engine verification
distinct. A parsed VPK or successful compiler exit is not proof of game behavior.

## Checks

```bash
cargo test -p vpkmerge-harness
cargo clippy -p vpkmerge-harness --all-targets --no-deps -- -D warnings
cargo fmt -p vpkmerge-harness --check
```

Desktop checks (from `gui/`):

```bash
pnpm vite:build
node --experimental-vm-modules --test tests/harness.test.mjs
```

The UI tests run real Vue component lifecycles with mocked Tauri calls. They cover
late replies after disconnect/port changes, picker completion after unmount,
capture prerequisites, and project switching. Native window appearance and real
Blender capture are separate manual checks.
