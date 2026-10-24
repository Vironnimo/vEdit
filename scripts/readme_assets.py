#!/usr/bin/env python
"""Render the README screenshots and write them as WebP to ``.github/assets/``.

The scenes come from the ignored Rust test ``app::readme_assets_tests::readme_assets``,
which builds demo projects in temporary Git repositories and renders vEdit
headlessly at two pixels per point. This script runs that test, then scales each
PNG to ``WIDTH`` pixels and stores it as WebP next to the README.

``python scripts/readme_assets.py`` regenerates every image. Review the result
before committing: the scenes run real Git and, when installed, rust-analyzer.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

from PIL import Image

PROJECT_ROOT = Path(__file__).resolve().parents[1]
RENDER_DIR = PROJECT_ROOT / ".tmp" / "readme-assets" / "png"
ASSETS_DIR = PROJECT_ROOT / ".github" / "assets"
SCENES = ("workspace", "agent-review", "change-review", "agent-marks")
WIDTH = 1760
"""Pixels wide: twice the README column on GitHub, so text stays sharp."""
QUALITY = 90


def render() -> None:
    RENDER_DIR.mkdir(parents=True, exist_ok=True)
    for stale in RENDER_DIR.glob("*.png"):
        stale.unlink()
    env = dict(os.environ, VEDIT_README_ASSETS_DIR=str(RENDER_DIR), WGPU_BACKEND="gl")
    # The demo repositories commit under a fixed demo identity.
    for name in ("GIT_AUTHOR_NAME", "GIT_AUTHOR_EMAIL", "GIT_COMMITTER_NAME", "GIT_COMMITTER_EMAIL"):
        env.pop(name, None)
    test = "app::readme_assets_tests::readme_assets"
    command = ["cargo", "test", "--bin", "vedit", test, "--", "--ignored", "--exact", test, "--test-threads=1"]
    subprocess.run(command, cwd=PROJECT_ROOT, env=env, check=True)


def convert() -> None:
    ASSETS_DIR.mkdir(parents=True, exist_ok=True)
    for scene in SCENES:
        with Image.open(RENDER_DIR / f"{scene}.png") as image:
            height = round(image.height * WIDTH / image.width)
            scaled = image.convert("RGB").resize((WIDTH, height), Image.Resampling.LANCZOS)
        target = ASSETS_DIR / f"{scene}.webp"
        scaled.save(target, "WEBP", quality=QUALITY, method=6)
        print(f"{target.relative_to(PROJECT_ROOT)}: {target.stat().st_size // 1024} KiB")


def main() -> int:
    render()
    convert()
    return 0


if __name__ == "__main__":
    sys.exit(main())
