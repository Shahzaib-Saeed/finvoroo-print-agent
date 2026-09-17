"""Generate PNG and ICO icons for the Finvoroo Print Agent from the pharmacy logo."""
from __future__ import annotations

import struct
import zlib
from pathlib import Path

from PIL import Image

AGENT_ROOT = Path(__file__).resolve().parents[1]
ICONS = AGENT_ROOT / "src-tauri" / "icons"
SRC_LOGO = AGENT_ROOT / "src" / "finvoroo-logo.png"
# Canonical Finvoroo pharmacy mark used across the ERP UI.
PHARMACY_LOGO = (
    AGENT_ROOT.parent
    / "React-frontend"
    / "public"
    / "media"
    / "app"
    / "pharmacy"
    / "logo.png"
)


def load_source() -> Image.Image:
    src = PHARMACY_LOGO if PHARMACY_LOGO.is_file() else SRC_LOGO
    if not src.is_file():
        raise SystemExit(f"Logo not found: tried {PHARMACY_LOGO} and {SRC_LOGO}")
    img = Image.open(src).convert("RGBA")
    print(f"Source logo: {src} ({img.size[0]}x{img.size[1]})")
    return img


def make_square_icon(src: Image.Image, size: int, *, pad_ratio: float = 0.12) -> Image.Image:
    """Letterbox the logo onto a square canvas (transparent), with a small inset."""
    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    inset = max(1, int(size * pad_ratio))
    box = size - inset * 2
    fitted = src.copy()
    fitted.thumbnail((box, box), Image.Resampling.LANCZOS)
    x = (size - fitted.width) // 2
    y = (size - fitted.height) // 2
    canvas.paste(fitted, (x, y), fitted)
    return canvas


def png_bytes(img: Image.Image) -> bytes:
    from io import BytesIO

    buf = BytesIO()
    img.save(buf, format="PNG", optimize=True)
    return buf.getvalue()


def ico_from_pngs(entries: list[tuple[int, bytes]]) -> bytes:
    """Build a multi-size ICO (PNG-compressed entries) for Windows installer + desktop."""
    count = len(entries)
    header = struct.pack("<HHH", 0, 1, count)
    offset = 6 + 16 * count
    dir_entries = b""
    data = b""
    for size, png in entries:
        dim = 0 if size >= 256 else size
        dir_entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(png), offset)
        data += png
        offset += len(png)
    return header + dir_entries + data


def main() -> None:
    ICONS.mkdir(parents=True, exist_ok=True)
    src = load_source()

    # Keep a crisp copy next to the agent UI for brand.js embedding / window chrome.
    ui_logo = make_square_icon(src, 256, pad_ratio=0.08)
    ui_logo.save(SRC_LOGO, format="PNG", optimize=True)

    p32 = make_square_icon(src, 32, pad_ratio=0.1)
    p48 = make_square_icon(src, 48, pad_ratio=0.1)
    p64 = make_square_icon(src, 64, pad_ratio=0.1)
    p128 = make_square_icon(src, 128, pad_ratio=0.1)
    p256 = make_square_icon(src, 256, pad_ratio=0.08)

    (ICONS / "32x32.png").write_bytes(png_bytes(p32))
    (ICONS / "128x128.png").write_bytes(png_bytes(p128))
    (ICONS / "icon.png").write_bytes(png_bytes(p256))

    ico = ico_from_pngs(
        [
            (32, png_bytes(p32)),
            (48, png_bytes(p48)),
            (64, png_bytes(p64)),
            (128, png_bytes(p128)),
            (256, png_bytes(p256)),
        ]
    )
    (ICONS / "icon.ico").write_bytes(ico)
    print(f"Wrote icons in {ICONS}")
    print(f"Wrote UI logo {SRC_LOGO}")


if __name__ == "__main__":
    main()
