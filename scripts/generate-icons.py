"""Generate PNG and ICO icons for the Finvoroo Print Agent."""
from __future__ import annotations

import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1] / "src-tauri" / "icons"


def png_bytes(size: int) -> bytes:
    def pixel(x: int, y: int) -> bytes:
        margin = max(2, size // 16)
        if x < margin or y < margin or x >= size - margin or y >= size - margin:
            r, g, b = 15, 39, 68
        else:
            bar = max(2, size // 10)
            cx, cy = size // 2, size // 2
            if abs(x - cx) < bar or (cy - bar * 2 < y < cy + bar and abs(x - cx) < size // 3):
                r, g, b = 2, 132, 199
            else:
                r, g, b = 15, 39, 68
        return bytes((r, g, b, 255))

    raw = b"".join(b"\x00" + b"".join(pixel(x, y) for x in range(size)) for y in range(size))

    def chunk(tag: bytes, data: bytes) -> bytes:
        crc = zlib.crc32(tag + data) & 0xFFFFFFFF
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", crc)

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def ico_from_png(png: bytes, size: int) -> bytes:
    header = struct.pack("<HHH", 0, 1, 1)
    dim = 0 if size >= 256 else size
    entry = struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(png), 6 + 16)
    return header + entry + png


def main() -> None:
    ROOT.mkdir(parents=True, exist_ok=True)
    p32 = png_bytes(32)
    p128 = png_bytes(128)
    p256 = png_bytes(256)
    (ROOT / "32x32.png").write_bytes(p32)
    (ROOT / "128x128.png").write_bytes(p128)
    (ROOT / "icon.png").write_bytes(p256)
    (ROOT / "icon.ico").write_bytes(ico_from_png(p256, 256))
    print(f"Wrote icons in {ROOT}")


if __name__ == "__main__":
    main()
