#!/usr/bin/env python3
"""Download the Title Fight source tracks for the Deadlock music kit.

Reads the resolved YouTube Music IDs in `source-tracks.json` and downloads each
track *by its exact video id* (no fuzzy search, so we never grab a full-album
upload or a live/cover by mistake). Audio lands in `source_audio/<key>.<ext>`,
matching the `local_audio` paths in `manifest.json`, which the pack builder then
trims/fades and mints into `.vsnd_c`.

`source_audio/` and all audio extensions are gitignored, so nothing copyrighted
is committed.

Usage:
    python3 download_songs.py                  # download every resolved track
    python3 download_songs.py --list           # show the plan + manifest coverage
    python3 download_songs.py --only frown leaf # subset by track key
    python3 download_songs.py --format mp3      # mp3 instead of wav (default wav)
    python3 download_songs.py --quality 320     # mp3 bitrate when --format mp3

Requires yt-dlp + ffmpeg on PATH (both present in this workspace).
Re-running is safe: already-downloaded tracks are skipped.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

import yt_dlp

HERE = Path(__file__).resolve().parent
SOURCE_TRACKS = HERE / "source-tracks.json"
MANIFEST = HERE / "manifest.json"
AUDIO_DIR = HERE / "source_audio"

WATCH_URL = "https://www.youtube.com/watch?v={vid}"
GENERATED_KEYS = {"silence"}


def load_tracks() -> list[dict]:
    data = json.loads(SOURCE_TRACKS.read_text())
    return data["tracks"]


def manifest_keys() -> set[str]:
    if not MANIFEST.exists():
        return set()
    data = json.loads(MANIFEST.read_text())
    return {e["source_track_key"] for e in data.get("entries", [])}


def print_plan(tracks: list[dict], only: set[str] | None) -> None:
    keys = {t["key"] for t in tracks} | GENERATED_KEYS
    used = manifest_keys()
    print(f"{'key':28} {'title':32} {'album':28} {'dur':>5}  id")
    for t in tracks:
        if only and t["key"] not in only:
            continue
        print(f"{t['key']:28} {t['title'][:31]:32} {t['album'][:27]:28} "
              f"{t.get('duration',''):>5}  {t['video_id']}")
    if not only or "silence" in only:
        print(f"{'silence':28} {'Generated silence':32} {'local utility':28} {'2s':>5}  generated")
    print(f"\nresolved tracks: {len(tracks)}")
    missing = sorted(used - keys)
    if missing:
        print(f"manifest keys with NO resolved id: {', '.join(missing)}")
    else:
        print("manifest coverage: every manifest track has a resolved id.")


def ensure_silence() -> None:
    AUDIO_DIR.mkdir(parents=True, exist_ok=True)
    out = AUDIO_DIR / "silence.wav"
    if out.exists():
        print(f"[skip] {out.name} (exists)")
        return
    print(f"[make] {out.name:32} Generated silence")
    subprocess.run([
        "ffmpeg",
        "-loglevel", "error",
        "-y",
        "-f", "lavfi",
        "-i", "anullsrc=channel_layout=stereo:sample_rate=48000",
        "-t", "2",
        str(out),
    ], check=True)


def download(tracks: list[dict], only: set[str] | None,
             fmt: str, quality: str) -> int:
    AUDIO_DIR.mkdir(parents=True, exist_ok=True)
    failures: list[str] = []
    plan = [t for t in tracks if not only or t["key"] in only]

    if not only or "silence" in only:
        try:
            ensure_silence()
        except Exception as exc:  # noqa: BLE001 - report and keep going
            print(f"[FAIL] silence: {exc}", file=sys.stderr)
            failures.append("silence")

    for t in plan:
        key = t["key"]
        out = AUDIO_DIR / f"{key}.{fmt}"
        if out.exists():
            print(f"[skip] {out.name} (exists)")
            continue

        url = WATCH_URL.format(vid=t["video_id"])
        print(f"[get ] {out.name:32} {t['title']} <- {t['video_id']}")

        postprocessors = [{
            "key": "FFmpegExtractAudio",
            "preferredcodec": fmt,
            **({"preferredquality": quality} if fmt == "mp3" else {}),
        }, {"key": "FFmpegMetadata"}]

        opts = {
            "format": "bestaudio/best",
            "outtmpl": str(AUDIO_DIR / f"{key}.%(ext)s"),
            "noplaylist": True,
            "quiet": True,
            "no_warnings": True,
            "postprocessors": postprocessors,
            "addmetadata": True,
        }
        try:
            with yt_dlp.YoutubeDL(opts) as ydl:
                ydl.download([url])
        except Exception as exc:  # noqa: BLE001 - report and keep going
            print(f"[FAIL] {key}: {exc}", file=sys.stderr)
            failures.append(key)
            continue
        if not out.exists():
            print(f"[FAIL] {key}: no {fmt} produced", file=sys.stderr)
            failures.append(key)

    print("\n=== done ===")
    print(f"requested: {len(plan)}   failed: {len(failures)}")
    if failures:
        for f in failures:
            print(f"  - {f}")
        return 1
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--list", action="store_true", help="print plan and exit")
    ap.add_argument("--only", nargs="*", metavar="KEY", help="only these track keys")
    ap.add_argument("--format", default="wav", choices=["wav", "mp3"],
                    help="output audio format (default wav, matches manifest)")
    ap.add_argument("--quality", default="320", help="mp3 bitrate when --format mp3")
    args = ap.parse_args()

    tracks = load_tracks()
    keys = {t["key"] for t in tracks} | GENERATED_KEYS
    only = set(args.only) if args.only else None
    if only:
        unknown = sorted(only - keys)
        if unknown:
            print(f"unknown key(s): {', '.join(unknown)}", file=sys.stderr)
            print(f"known: {', '.join(sorted(keys))}", file=sys.stderr)
            return 2

    if args.list:
        print_plan(tracks, only)
        return 0

    return download(tracks, only, args.format, args.quality)


if __name__ == "__main__":
    raise SystemExit(main())
