#!/usr/bin/env python3
"""Regenerate the lossless embedded GPU asset (Pillow required only for this tool)."""
from pathlib import Path
from PIL import Image

assets = Path(__file__).resolve().parents[1] / "assets"
image = Image.open(assets / "loading_screen.png").convert("RGBA")
if image.size != (1672, 941):
    raise SystemExit("Update IMAGE_WIDTH/IMAGE_HEIGHT in src/loading.rs for a new image size")
(assets / "loading_screen.bgra").write_bytes(image.tobytes("raw", "BGRA"))
