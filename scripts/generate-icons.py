"""Generate PNG and ICO icons for Finvoroo Print Agent from the universal Finvoroo mark."""
from __future__ import annotations

import struct
from io import BytesIO
from pathlib import Path

from PIL import Image

AGENT_ROOT = Path(__file__).resolve().parents[1]
ICONS = AGENT_ROOT / "src-tauri" / "icons"
UI_LOGO = AGENT_ROOT / "src" / "finvoroo-logo.png"

UNIVERSAL_LOGO_CANDIDATES = [
    AGENT_ROOT.parent / "React-frontend" / "public" / "media" / "app" / "universal" / "logo.png",
    AGENT_ROOT / "React-frontend" / "public" / "media" / "app" / "universal" / "logo.png",
    AGENT_ROOT / "src" / "finvoroo-logo.png",
]


def load_source() -> Image.Image:
    src = next((p for p in UNIVERSAL_LOGO_CANDIDATES if p.is_file()), None)
    if src is None:
        raise SystemExit(
            "Finvoroo mark not found. Expected React-frontend/public/media/app/universal/logo.png"
        )
    img = Image.open(src).convert("RGBA")
    side = min(img.size)
    if img.size[0] != img.size[1]:
        left = max(0, (img.size[0] - side) // 2)
        top = max(0, (img.size[1] - side) // 2)
        img = img.crop((left, top, left + side, top + side))
    print(f"Source logo: {src} ({img.size[0]}x{img.size[1]})")
    return img


def make_square_icon(src: Image.Image, size: int, *, pad_ratio: float = 0.1) -> Image.Image:
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
    buf = BytesIO()
    img.save(buf, format="PNG", optimize=True)
    return buf.getvalue()


def ico_from_pngs(entries: list[tuple[int, bytes]]) -> bytes:
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

    ui_logo = make_square_icon(src, 256, pad_ratio=0.06)
    ui_logo.save(UI_LOGO, format="PNG", optimize=True)

    sizes = [32, 48, 64, 128, 256]
    pngs = {size: make_square_icon(src, size, pad_ratio=0.08 if size <= 64 else 0.06) for size in sizes}

    (ICONS / "32x32.png").write_bytes(png_bytes(pngs[32]))
    (ICONS / "128x128.png").write_bytes(png_bytes(pngs[128]))
    (ICONS / "icon.png").write_bytes(png_bytes(pngs[256]))
    (ICONS / "icon.ico").write_bytes(
        ico_from_pngs([(s, png_bytes(pngs[s])) for s in sizes])
    )
    print(f"Wrote icons in {ICONS}")
    print(f"Wrote UI logo {UI_LOGO}")


if __name__ == "__main__":
    main()
