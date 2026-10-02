// Builds the vpkmerge-mcp server from the vpkmerge checkout and puts it where
// Tauri's `externalBin` expects it: src-tauri/binaries/vpkmerge-mcp-<target triple>.
// The checkout defaults to ../vpkmerge; point VPKMERGE_DIR somewhere else to override.

import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const vpkmerge = resolve(process.env.VPKMERGE_DIR ?? join(root, "..", "vpkmerge"));
const manifest = join(vpkmerge, "Cargo.toml");
if (!existsSync(manifest)) {
  console.error(`No vpkmerge checkout at ${vpkmerge}. Clone it there or set VPKMERGE_DIR.`);
  process.exit(1);
}

const cargo = (args, options) => execFileSync("cargo", [...args, "--manifest-path", manifest], options);

const host = execFileSync("rustc", ["-vV"], { encoding: "utf8" }).match(/^host: (.+)$/m)[1];
// Tauri sets this for `before*Command` hooks when building for another target.
const triple = process.env.TAURI_ENV_TARGET_TRIPLE ?? host;
const cross = triple !== host;

cargo(["build", "--release", "-p", "vpkmerge-mcp", ...(cross ? ["--target", triple] : [])], { stdio: "inherit" });

const metadata = JSON.parse(cargo(["metadata", "--format-version", "1", "--no-deps"], { encoding: "utf8" }));
const ext = triple.includes("windows") ? ".exe" : "";
const built = join(metadata.target_directory, cross ? triple : "", "release", `vpkmerge-mcp${ext}`);
const dest = join(root, "src-tauri", "binaries", `vpkmerge-mcp-${triple}${ext}`);
mkdirSync(dirname(dest), { recursive: true });
copyFileSync(built, dest);
console.log(`sidecar: ${dest}`);
