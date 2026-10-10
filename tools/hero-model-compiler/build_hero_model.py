#!/usr/bin/env python3
"""Compile a rigged GLB (mesh + skeleton + skin) into a Source 2 hero-model
addon VPK via Valve's resourcecompiler under Proton.

Linux/Proton wrapper, adapted from the soul-container compiler. Stages skinned
content with Blender (prepare_hero_blender.py), compiles through the Deadlock
CSDK, then packs the compiled game addon tree into a dir VPK at the donor hero's
model path so the engine renders the swapped mesh on the donor's skeleton.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
BLENDER_STAGE = HERE / "prepare_hero_blender.py"
DEFAULT_CSDK_ROOT = Path(os.environ.get("CSDK_ROOT", "/home/esoc/csdk12/Reduced_CSDK_12"))
DEFAULT_PROTON = Path(
    os.environ.get(
        "PROTON",
        "/home/esoc/.local/share/Steam/steamapps/common/Proton - Experimental/proton",
    )
)
DEFAULT_STEAM_ROOT = Path(os.environ.get("STEAM_ROOT", "/home/esoc/.local/share/Steam"))
DEFAULT_PROTON_PREFIX = Path(os.environ.get("STEAM_COMPAT_DATA_PATH", "/tmp/proton-vpkmerge-rc"))


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description="Compile a rigged GLB into a Source 2 hero-model addon VPK.")
    p.add_argument("input_glb", type=Path)
    p.add_argument("--addon", default="bmo_familiar")
    p.add_argument("--model-rel", default="models/heroes_wip/familiar")
    p.add_argument("--model-name", default="familiar")
    p.add_argument("--target-source-height", type=float, default=109.0)
    p.add_argument("--source-units-per-blender", type=float, default=100.0)
    p.add_argument("--facing-yaw", type=float, default=0.0,
                   help="degrees about Z to rotate the rig so it faces Source +X "
                        "(e.g. 90 / -90 / 180); 0 = no rotation")
    p.add_argument("--donor-vpk", type=Path, default=None,
                   help="base pak (e.g. pak01_dir.vpk) to copy the donor hero's full "
                        "skeleton from; without it RC culls bones to the skin-weighted "
                        "set and the animgraph can't drive the hierarchy")
    p.add_argument("--anim-graph", default="",
                   help="animgraph resource to drive the model (RootNode anim_graph_name), "
                        "e.g. animgraphs/animgraph2/hero/hero.vnmgraph+familiar.vnmgraph")
    p.add_argument("--codename", default="",
                   help="donor hero codename for the post-compile NM-ref injection "
                        "(animgraph + .vnmskel), e.g. familiar. Required for animation.")
    p.add_argument("--skeleton-json", type=Path, default=None,
                   help="pre-built skeleton JSON (dump_skeleton shape) to emit into the "
                        "staged .vmdl instead of dumping the donor's. Use for merged "
                        "skeletons (e.g. merge_skeletons.py output when the GLB carries "
                        "custom bones the donor lacks). --donor-vpk still supplies the "
                        "flag/keyValueText refs.")
    p.add_argument("--flags-ref2", type=Path, default=None,
                   help="second fix_bone_flags reference: a loose .vmdl_c whose bones "
                        "cover names absent from the --donor-vpk model (e.g. the other "
                        "half of a merged skeleton). Runs after the donor flag pass.")
    p.add_argument("--camera-ref", type=Path, default=None,
                   help="loose .vmdl_c whose MDAT camera attachments (near/far/gunaim/"
                        "pivots) are retargeted onto the compiled model post-compile "
                        "(patch_camera_attachments), fixing the over-the-shoulder camera "
                        "headless RC loses precision on.")
    p.add_argument("--rebind-json", type=Path, default=None,
                   help="dump_skeleton JSON of the runtime skeleton (e.g. the vanilla "
                        "hero's). Blender staging poses matching bones to these binds, "
                        "bakes the deformation into the mesh, and applies the pose as "
                        "rest, so animated bones anchor where the engine drives them. "
                        "Bones absent from the JSON ride their corrected parents.")
    p.add_argument("--skeleton-from-rig", action="store_true",
                   help="emit the .vmdl Skeleton from the staged rig's own rest pose "
                        "(_rig_skeleton.json written by the Blender stage) instead of "
                        "--skeleton-json / the donor dump. Guarantees the engine binds "
                        "equal the FBX binds RC skins the mesh against.")
    p.add_argument("--seed-game-tree", type=Path, default=None,
                   help="directory tree copied into the addon's game dir before the RC "
                        "compile. Use for compiled materials (.vmat_c + .vtex_c) the "
                        "model references but no mounted pak ships: RC drops material "
                        "refs it cannot resolve (the covered draw calls merge into one "
                        "empty-material call). Seeded files also end up in the packed "
                        "VPK.")
    p.add_argument("--physics-from-donor", action="store_true",
                   help="give the model the donor hero's PHYS (ragdoll bodies + joints, "
                        "FeModel cloth). Stages a one-capsule PhysicsShapeList so RC emits "
                        "and registers a PHYS block, then swaps the donor's PHYS bytes into "
                        "it (replace_phys). Requires --donor-vpk; the skeleton must come from "
                        "the same pak or cloth bones added by a game update are missing.")
    p.add_argument("--lods", default="",
                   help="LOD chain RC generates by simplifying the mesh, as "
                        "SWITCH_DISTANCE:TRIANGLE_FRACTION pairs, e.g. 14:0.7,35:0.25 "
                        "(vanilla heroes switch at 14 and 35). Default: LOD0 only.")
    p.add_argument("--distance-fields-from-donor", action="store_true",
                   help="have RC build per-bone distance fields (DSTF, the hero's AO "
                        "occluders) from the swapped mesh, on the bones the donor uses "
                        "(dstf_bones). Requires --donor-vpk.")
    p.add_argument("--output", type=Path, default=None)
    p.add_argument("--csdk-root", type=Path, default=DEFAULT_CSDK_ROOT)
    p.add_argument("--proton", type=Path, default=DEFAULT_PROTON)
    p.add_argument("--steam-root", type=Path, default=DEFAULT_STEAM_ROOT)
    p.add_argument("--proton-prefix", type=Path, default=DEFAULT_PROTON_PREFIX)
    p.add_argument("--blender", default=os.environ.get("BLENDER", "blender"))
    p.add_argument("--force", action="store_true")
    p.add_argument("--stage-only", action="store_true",
                   help="stage the content .vmdl + FBX into the CSDK content tree and STOP "
                        "before the headless compile. Use when the model must be compiled in "
                        "the ModelDoc GUI (Deadlock_with_tools.exe) instead -- the GUI bakes "
                        "the AttachmentCameraData hero camera that headless resourcecompiler "
                        "drops. Finish with --finish-only after the GUI writes the .vmdl_c.")
    p.add_argument("--finish-only", action="store_true",
                   help="skip Blender staging + compile; run only the post-compile injections "
                        "(animgraph + bone flags + keyValueText) on an already-compiled .vmdl_c "
                        "(e.g. one the ModelDoc GUI produced) and pack the addon VPK.")
    p.add_argument("--keep-staging", action="store_true")
    p.add_argument("--install-to", type=Path, default=None)
    return p.parse_args()


def run(cmd, *, cwd=None, env=None) -> None:
    print("+ " + " ".join(str(c) for c in cmd), flush=True)
    subprocess.run(cmd, cwd=cwd, env=env, check=True)


def wine_z_path(path: Path) -> str:
    return "Z:" + str(path.resolve()).replace("/", "\\")


def require_file(path: Path, label: str) -> None:
    if not path.is_file():
        raise SystemExit(f"{label} not found: {path}")


def require_dir(path: Path, label: str) -> None:
    if not path.is_dir():
        raise SystemExit(f"{label} not found: {path}")


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def remove_staging(path: Path, *, force: bool) -> None:
    if not path.exists():
        return
    if not force:
        raise SystemExit(f"staging path exists; rerun with --force to remove it: {path}")
    shutil.rmtree(path)


def append_root_nodes(vmdl: Path, nodes: str) -> None:
    s = vmdl.read_text(encoding="utf-8")
    end = s.rfind("\t\t]\n\t}\n}")
    if end < 0:
        raise SystemExit(f"cannot find the RootNode children end in {vmdl}")
    vmdl.write_text(s[:end] + nodes + s[end:], encoding="utf-8", newline="\n")


# Any PhysicsShapeList makes RC emit a PHYS block and register it in CTRL
# (embedded_physics). The capsule itself is discarded: replace_phys swaps the
# donor's PHYS bytes into that registered block after the compile.
PHYS_STUB_NODE = (
    "\t\t\t{\n\t\t\t\t_class = \"PhysicsShapeList\"\n\t\t\t\tchildren =\n\t\t\t\t[\n"
    "\t\t\t\t\t{\n\t\t\t\t\t\t_class = \"PhysicsShapeCapsule\"\n"
    "\t\t\t\t\t\tsurface_prop = \"default\"\n\t\t\t\t\t\tcollision_tags = \"\"\n"
    "\t\t\t\t\t\tradius = 1.0\n\t\t\t\t\t\tpoint0 = [ 0.0, 0.0, 0.0 ]\n"
    "\t\t\t\t\t\tpoint1 = [ 0.0, 0.0, 1.0 ]\n\t\t\t\t\t\tname = \"\"\n"
    "\t\t\t\t\t},\n\t\t\t\t]\n\t\t\t},\n"
)


def lod_nodes(model_name: str, spec: str) -> str:
    # copy_and_simplify has RC duplicate + decimate the LOD0 mesh itself;
    # targetTrianglePercent is a 0..1 fraction despite its name.
    groups = [(0.0, "")]
    for pair in spec.split(","):
        dist, frac = pair.split(":")
        groups.append((float(dist), (
            " copy_and_simplify = true simplify_params = "
            f"{{ targetTrianglePercent = {float(frac)} weldVertices = true }}")))
    body = "".join(
        "\t\t\t\t\t{\n\t\t\t\t\t\t_class = \"LODGroup\"\n"
        f"\t\t\t\t\t\tswitch_threshold = {dist}\n"
        f"\t\t\t\t\t\tmesh_references = [ {{ mesh_name = \"{model_name}\"{simplify} }} ]\n"
        "\t\t\t\t\t},\n"
        for dist, simplify in groups)
    return ("\t\t\t{\n\t\t\t\t_class = \"LODGroupList\"\n\t\t\t\tchildren =\n\t\t\t\t[\n"
            + body + "\t\t\t\t]\n\t\t\t},\n")


def distance_field_nodes(model_name: str, bones: list[str]) -> str:
    body = "".join(
        "\t\t\t\t\t{\n\t\t\t\t\t\t_class = \"DistanceField\"\n"
        f"\t\t\t\t\t\tparent_bone = \"{bone}\"\n"
        f"\t\t\t\t\t\tfilter_bones = [ \"{bone}\" ]\n"
        "\t\t\t\t\t\tuse_for_occlusion = true\n\t\t\t\t\t\tsurface_bias = 0.5\n"
        "\t\t\t\t\t\tchildren =\n\t\t\t\t\t\t[\n"
        "\t\t\t\t\t\t\t{\n\t\t\t\t\t\t\t\t_class = \"DistanceFieldFromRender\"\n"
        f"\t\t\t\t\t\t\t\tmeshes = [ \"{model_name}\" ]\n\t\t\t\t\t\t\t}},\n"
        "\t\t\t\t\t\t]\n\t\t\t\t\t},\n"
        for bone in bones)
    return ("\t\t\t{\n\t\t\t\t_class = \"DistanceFieldList\"\n\t\t\t\tchildren =\n\t\t\t\t[\n"
            + body + "\t\t\t\t]\n\t\t\t},\n")


def extract_donor_model(args: argparse.Namespace, dest: Path) -> Path:
    if not dest.is_file():
        run(["cargo", "run", "--release", "-q", "-p", "vpkmerge-core", "--example",
             "extract_entry", "--", str(args.donor_vpk.resolve()),
             f"{args.model_rel}/{args.model_name}.vmdl_c", str(dest)], cwd=REPO_ROOT)
    return dest


def main() -> int:
    args = parse_args()
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", args.addon):
        raise SystemExit(f"addon name must be file-name safe: {args.addon!r}")
    if (args.physics_from_donor or args.distance_fields_from_donor) and not args.donor_vpk:
        raise SystemExit("--physics-from-donor / --distance-fields-from-donor need --donor-vpk")

    input_glb = args.input_glb.resolve()
    csdk_root = args.csdk_root.resolve()
    output = (args.output or (REPO_ROOT / "target" / f"{args.addon}_dir.vpk")).resolve()
    content_addon = csdk_root / "content" / "citadel_addons" / args.addon
    game_addon = csdk_root / "game" / "citadel_addons" / args.addon
    source_dir = content_addon / args.model_rel
    compiler_dir = csdk_root / "game" / "bin_tools" / "win64"

    require_file(input_glb, "input GLB")
    require_file(args.proton, "Proton executable")
    require_file(BLENDER_STAGE, "Blender stage script")
    require_dir(compiler_dir, "resourcecompiler directory")

    # --finish-only reuses the GUI-compiled game_addon (and the staged content
    # from a prior --stage-only run); never wipe them.
    if not args.finish_only:
        remove_staging(content_addon, force=args.force)
        remove_staging(game_addon, force=args.force)
    source_dir.mkdir(parents=True, exist_ok=True)
    output.parent.mkdir(parents=True, exist_ok=True)
    source_vmdl = source_dir / f"{args.model_name}.vmdl"

    if not args.finish_only and args.seed_game_tree:
        seed = args.seed_game_tree.resolve()
        require_dir(seed, "seed game tree")
        shutil.copytree(seed, game_addon, dirs_exist_ok=True)
        print(f"seeded game tree from {seed}")

    if not args.finish_only:
        env = os.environ.copy()
        env["S2_MODEL_REL"] = args.model_rel
        env["S2_MODEL_NAME"] = args.model_name
        env["S2_TARGET_SOURCE_HEIGHT"] = str(args.target_source_height)
        env["S2_SOURCE_UNITS_PER_BLENDER"] = str(args.source_units_per_blender)
        env["S2_FACING_YAW_DEG"] = str(args.facing_yaw)
        if args.rebind_json:
            require_file(args.rebind_json, "rebind skeleton JSON")
            env["S2_REBIND_JSON"] = str(args.rebind_json.resolve())

        run([args.blender, "--background", "--python", str(BLENDER_STAGE), "--",
             str(input_glb), str(source_dir)], env=env)

        require_file(source_vmdl, "generated source model")

        # Overwrite the staged simple .vmdl with one carrying the donor hero's full
        # skeleton (correct Source-space transforms). Otherwise RC keeps only the
        # skin-weighted bones and drops the parent chains the animgraph needs.
        # --skeleton-json supplies the bone list directly (a merged skeleton);
        # otherwise it is dumped from the donor VPK.
        if args.skeleton_from_rig or args.skeleton_json or args.donor_vpk:
            if args.skeleton_from_rig:
                skel_json = source_dir / "_rig_skeleton.json"
                require_file(skel_json, "staged rig skeleton JSON")
            elif args.skeleton_json:
                skel_json = args.skeleton_json.resolve()
                require_file(skel_json, "skeleton JSON")
            else:
                donor_entry = f"{args.model_rel}/{args.model_name}.vmdl_c"
                skel_json = source_dir / "_donor_skeleton.json"
                with skel_json.open("w") as f:
                    subprocess.run(
                        ["cargo", "run", "--release", "-q", "-p", "vpkmerge-core",
                         "--example", "dump_skeleton", "--",
                         str(args.donor_vpk.resolve()), donor_entry],
                        cwd=REPO_ROOT, check=True, stdout=f)
            run([sys.executable, str(HERE / "emit_skeleton_vmdl.py"),
                 str(skel_json), str(source_vmdl), args.model_rel, args.model_name,
                 args.anim_graph])
            if not args.skeleton_json and not args.skeleton_from_rig:
                skel_json.unlink(missing_ok=True)

        # ModelDoc nodes a bare mesh compile lacks: PHYS stub (filled from the
        # donor post-compile), RC-simplified LOD chain, per-bone distance fields.
        extra_nodes = ""
        if args.physics_from_donor:
            extra_nodes += PHYS_STUB_NODE
        if args.lods:
            extra_nodes += lod_nodes(args.model_name, args.lods)
        if args.distance_fields_from_donor:
            donor_model = extract_donor_model(args, source_dir / "_donor_extras.vmdl_c")
            bones = subprocess.run(
                ["cargo", "run", "--release", "-q", "-p", "morphic", "--example",
                 "dstf_bones", "--", str(donor_model)],
                cwd=REPO_ROOT, check=True, stdout=subprocess.PIPE, text=True).stdout.split()
            if bones:
                extra_nodes += distance_field_nodes(args.model_name, bones)
            else:
                print("donor model has no distance fields; skipping DSTF")
        if extra_nodes:
            append_root_nodes(source_vmdl, extra_nodes)

        if args.stage_only:
            win_vmdl = wine_z_path(source_vmdl)
            print("\n=== STAGED FOR MODELDOC GUI COMPILE ===")
            print(f"source .vmdl (Linux): {source_vmdl}")
            print(f"source .vmdl (Wine Z:): {win_vmdl}")
            print("Open it in Deadlock_with_tools.exe -> ModelDoc, then Compile.")
            print("The GUI bakes the AttachmentCameraData hero camera that headless")
            print("resourcecompiler drops. After it writes the .vmdl_c under")
            print(f"  {game_addon / args.model_rel}")
            print("rerun this command with --finish-only (same args) to inject the")
            print("animgraph/flags/keyValueText and pack the addon VPK.")
            return 0

        with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False, encoding="utf-8") as f:
            filelist = Path(f.name)
            f.write(wine_z_path(source_vmdl) + "\n")

        rc_env = os.environ.copy()
        rc_env.update({
            "STEAM_COMPAT_DATA_PATH": str(args.proton_prefix),
            "STEAM_COMPAT_CLIENT_INSTALL_PATH": str(args.steam_root),
            "SteamAppId": "1422450",
            "SteamGameId": "1422450",
            "VPROJECT": "1",
        })

        # NOTE: the Proton runtime (post 2026-06 update) no longer resolves a
        # cwd-relative exe name ("Failed to create process ...: 2"); pass the exe by
        # absolute Z: path.
        rc_exe_z = wine_z_path(compiler_dir / "resourcecompiler.exe")
        try:
            run([str(args.proton), "run", rc_exe_z,
                 "-game", "citadel", "-addon", args.addon,
                 "-fshallow", "-nop4", "-v", "-consoleapp", "-consolelog",
                 "-condebug", "-toconsole",
                 "-danger_mode_ignore_schema_mismatches",
                 "-filelist", wine_z_path(filelist)],
                cwd=compiler_dir, env=rc_env)
        finally:
            filelist.unlink(missing_ok=True)

    require_dir(game_addon, "compiled game addon")

    # Inject the NM animgraph refs the headless compiler omits (m_animGraph2Refs
    # + m_vecNmSkeletonRefs). These are what make the NM motion-matching graph
    # tick on the swapped mesh; resourcecompiler won't emit them from a
    # hand-authored .vmdl, so patch them byte-faithfully into the compiled DATA
    # (a full re-encode would drop the skeleton's typed tags -> red wireframe).
    if args.anim_graph and args.codename:
        compiled_vmdl_c = game_addon / args.model_rel / f"{args.model_name}.vmdl_c"
        require_file(compiled_vmdl_c, "compiled .vmdl_c to inject")
        run(["cargo", "run", "--release", "-p", "morphic", "--example",
             "inject_animgraph", "--",
             str(compiled_vmdl_c), str(compiled_vmdl_c),
             args.codename, args.model_rel], cwd=REPO_ROOT)

    # Copy the donor hero's true per-bone m_modelSkeleton.m_nFlag onto the
    # compiled model, matched by bone name. A mesh-only RC compile stamps every
    # bone with a uniform deform flag (Mesh|VertexLod0..7); the real model gives
    # each bone its Animation/Attachment/Procedural(twist)/Cloth/Hitbox/Physics
    # bits and only the LODs it actually deforms. Wrong flags make the swap
    # "animate but subtly wrong" (twist bones don't follow, etc). Byte-faithful
    # in-place scalar patch -- no re-encode, so the skeleton's typed tags and the
    # injected animgraph refs are preserved.
    if args.donor_vpk:
        compiled_vmdl_c = game_addon / args.model_rel / f"{args.model_name}.vmdl_c"
        require_file(compiled_vmdl_c, "compiled .vmdl_c to fix flags")
        ref_vmdl_c = source_dir / "_donor_model.vmdl_c"
        donor_entry = f"{args.model_rel}/{args.model_name}.vmdl_c"
        run(["cargo", "run", "--release", "-q", "-p", "vpkmerge-core", "--example",
             "extract_entry", "--",
             str(args.donor_vpk.resolve()), donor_entry, str(ref_vmdl_c)],
            cwd=REPO_ROOT)
        run(["cargo", "run", "--release", "-p", "morphic", "--example",
             "fix_bone_flags", "--",
             str(compiled_vmdl_c), str(ref_vmdl_c), str(compiled_vmdl_c)], cwd=REPO_ROOT)
        # Copy the donor's m_modelInfo.m_keyValueText (the vmdlkeys4 blob) onto the
        # model: its BoneConstraintList (twist bones), ikdata/FeetSettings (IK +
        # foot plant) and LookAtList (head/neck) are the runtime procedural rig,
        # all bone-referenced by name. A mesh-only compile ships an almost-empty
        # blob, so those bones stay at bind -> warped forearms/fingers, head offset
        # from neck. The donor blob is a superset of our animgraph stub, so this
        # wholesale copy restores the rig and keeps the animgraph wiring.
        run(["cargo", "run", "--release", "-p", "morphic", "--example",
             "inject_keyvaluetext", "--",
             str(compiled_vmdl_c), str(ref_vmdl_c), str(compiled_vmdl_c)], cwd=REPO_ROOT)
        ref_vmdl_c.unlink(missing_ok=True)

    # Second bone-flag pass: a merged skeleton's extra bones are absent from the
    # donor model, so the first pass leaves them at RC's uniform deform flag.
    # A second reference covering those names restores the authored flags.
    if args.flags_ref2:
        compiled_vmdl_c = game_addon / args.model_rel / f"{args.model_name}.vmdl_c"
        require_file(compiled_vmdl_c, "compiled .vmdl_c to fix flags (pass 2)")
        require_file(args.flags_ref2, "second flag reference .vmdl_c")
        run(["cargo", "run", "--release", "-p", "morphic", "--example",
             "fix_bone_flags", "--",
             str(compiled_vmdl_c), str(args.flags_ref2.resolve()),
             str(compiled_vmdl_c)], cwd=REPO_ROOT)

    if args.physics_from_donor:
        compiled_vmdl_c = game_addon / args.model_rel / f"{args.model_name}.vmdl_c"
        require_file(compiled_vmdl_c, "compiled .vmdl_c to fill PHYS")
        donor_model = extract_donor_model(args, source_dir / "_donor_extras.vmdl_c")
        run(["cargo", "run", "--release", "-p", "morphic", "--example",
             "replace_phys", "--",
             str(compiled_vmdl_c), str(donor_model), str(compiled_vmdl_c)], cwd=REPO_ROOT)

    # Retarget the MDAT camera attachments (over-the-shoulder anchors) to a
    # known-good reference's exact f32 transforms. The staged AttachmentList only
    # carries euler-rounded values, and headless RC drops AttachmentCameraData,
    # so this pins the camera to the reference hero's real transforms.
    if args.camera_ref:
        compiled_vmdl_c = game_addon / args.model_rel / f"{args.model_name}.vmdl_c"
        require_file(compiled_vmdl_c, "compiled .vmdl_c to patch camera")
        require_file(args.camera_ref, "camera reference .vmdl_c")
        run(["cargo", "run", "--release", "-p", "morphic", "--example",
             "patch_camera_attachments", "--", "patch",
             str(compiled_vmdl_c), str(args.camera_ref.resolve()),
             str(compiled_vmdl_c)], cwd=REPO_ROOT)

    run(["cargo", "run", "--release", "-p", "vpkmerge-core", "--example",
         "pack_tree", "--", str(game_addon), str(output)], cwd=REPO_ROOT)

    digest = sha256(output)
    print(f"built {output}")
    print(f"sha256 {digest}")

    if args.install_to:
        install_to = args.install_to.resolve()
        install_to.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(output, install_to)
        print(f"installed {install_to}")

    if not args.keep_staging:
        shutil.rmtree(content_addon, ignore_errors=True)
        shutil.rmtree(game_addon, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
