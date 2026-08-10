#!/usr/bin/env python3
"""HAND-RUN GENERATOR — not invoked by the build.

Sibling of ``make_tray_icon.swift`` (which draws the macOS menubar template
PNG). Nothing in ``build.rs``, ``Cargo.toml`` or ``packaging/make-app.sh``
calls this file. Run it manually; the outputs are committed:

    .venv/bin/python icons/make_windows_icons.py

Produces, from the same artwork as ``icons/icon.png``:

  * ``icons/icon.ico``            — the Windows application icon. ``tauri-build``
    hard-fails without it ("`icons/icon.ico` not found; required for generating
    a Windows Resource file during tauri-build"), so M6 cannot start until this
    file exists. Embedded as resource id 32512 (``IDI_APPLICATION``) and used by
    Explorer, the taskbar and Alt-Tab.
  * ``icons/tray-windows-{16,20,24,32}.png`` — the Windows tray glyph, one file
    per DPI step.

**The .ico has nothing to do with the tray glyph.** Tauri's public tray API only
exposes ``Icon::from_rgba``, which calls Win32 ``CreateIcon`` at the source
image's exact pixel dimensions; it never reads the ``.ico`` and never calls
``LoadIconMetric``. So the tray icon is whatever PNG the Rust code passes at
runtime, scaled by the shell if it is the wrong size. Handing Windows the 44x44
macOS ``tray.png`` would render blurry at the 16 px base size — hence the
separate per-DPI PNGs. Windows' small-icon DPI ladder is 16 @ 100%, 20 @ 125%,
24 @ 150%, 32 @ 200%.

The macOS ``tray.png`` is *also* wrong on Windows for a second reason: it is a
black-on-transparent template image, which macOS recolours for the menubar and
Windows does not. Pure black is invisible on the default dark taskbar. These
assets are therefore the full-colour blue tile with a knocked-out white "A" —
the same identity as ``icon.png``, and legible on a light *and* a dark taskbar
without a theme-swap.

Every size is redrawn at its target resolution rather than downscaled from one
master: the corner radius and the glyph's proportion of the tile are recomputed
per size, and the small sizes get a deliberately larger, heavier "A" that would
turn to mush if the 256 px artwork were merely resampled.
"""

import struct
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ICONS = Path(__file__).resolve().parent

# Sampled from the committed icons/icon.png, so the generated sizes are the
# same artwork rather than a lookalike.
BLUE = (67, 111, 195, 255)
WHITE = (255, 255, 255, 255)

# Corner radius as a fraction of the tile: icon.png's rounded rect first goes
# opaque 96 px into a 512 px edge.
RADIUS_FRAC = 0.1875

# Vertical centre of the glyph as a fraction of the tile. icon.png's "A" ink
# box is y 168..378 of 512, i.e. centred slightly below the geometric middle.
GLYPH_CY_FRAC = 0.5332

FONT = "/System/Library/Fonts/Supplemental/Arial Bold.ttf"

# Height of the "A" ink box as a fraction of the tile, per size.
#
# 0.41 is icon.png's own proportion and is right once there are enough pixels
# to carry it. Below ~40 px it leaves the letter too small to read in a tray
# slot, so the glyph grows as the tile shrinks — the standard small-icon
# compensation, and the reason these are redraws and not resamples.
GLYPH_H_FRAC = {16: 0.56, 20: 0.52, 24: 0.50, 32: 0.46, 40: 0.43}
GLYPH_H_DEFAULT = 0.4102

# Sizes Microsoft specifies for a Win32 application icon: the classic set
# (16/24/32/48/64) plus the high-dpi small ladder (20/40) and 256 for Explorer's
# extra-large view.
ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 256]

# Windows' small-icon DPI ladder: 100% / 125% / 150% / 200%.
TRAY_SIZES = [16, 20, 24, 32]


def render(size: int) -> Image.Image:
    """Draws one tile at exactly `size` x `size` pixels.

    Supersampled and resampled down once at the end, so the anti-aliasing is
    computed against the final pixel grid rather than inherited from a larger
    master.
    """
    ss = 16 if size <= 64 else 4
    big = size * ss
    img = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    draw.rounded_rectangle(
        [(0, 0), (big - 1, big - 1)],
        radius=RADIUS_FRAC * big,
        fill=BLUE,
    )

    target_h = GLYPH_H_FRAC.get(size, GLYPH_H_DEFAULT) * big

    # Arial's "A" ink height is a fixed ratio of the nominal font size, but
    # which ratio is a font-internal detail — so measure it once at a probe
    # size, scale linearly, then re-measure to correct any rounding.
    probe = ImageFont.truetype(FONT, 100)
    _, top, _, bottom = probe.getbbox("A")
    font = ImageFont.truetype(FONT, max(1, round(100 * target_h / (bottom - top))))

    left, top, right, bottom = font.getbbox("A")
    # getbbox is relative to the text anchor, so subtracting it places the ink
    # box — not the em box, whose side bearings and descender space would throw
    # the centring off — exactly where we want it.
    draw.text(
        (
            big / 2 - (left + right) / 2,
            GLYPH_CY_FRAC * big - (top + bottom) / 2,
        ),
        "A",
        font=font,
        fill=WHITE,
    )

    return img.resize((size, size), Image.LANCZOS)


def bmp_payload(img: Image.Image) -> bytes:
    """One .ico entry in 32-bit BGRA DIB form.

    Microsoft's icon spec wants 32-bit-with-alpha for every size, and PNG
    compression only for the 256 px image. Written by hand rather than through
    Pillow's ICO writer so each entry's encoding is chosen here rather than
    inferred.
    """
    w, h = img.size
    px = img.load()

    # The XOR bitmap is bottom-up and BGRA, per BITMAPINFOHEADER.
    xor = bytearray()
    for y in range(h - 1, -1, -1):
        for x in range(w):
            r, g, b, a = px[x, y]
            xor += bytes((b, g, r, a))

    # The AND (transparency) mask is 1 bpp with rows padded to 4 bytes. Windows
    # ignores it when the XOR bitmap carries alpha, but the spec still requires
    # it to be present and sized, so it is written as all-zero (= all opaque).
    and_row = ((w + 31) // 32) * 4
    and_mask = bytes(and_row * h)

    header = struct.pack(
        "<IiiHHIIiiII",
        40,  # biSize
        w,  # biWidth
        h * 2,  # biHeight — XOR and AND bitmaps stacked
        1,  # biPlanes
        32,  # biBitCount
        0,  # biCompression = BI_RGB
        len(xor) + len(and_mask),  # biSizeImage
        0,  # biXPelsPerMeter
        0,  # biYPelsPerMeter
        0,  # biClrUsed
        0,  # biClrImportant
    )
    return header + bytes(xor) + and_mask


def png_payload(img: Image.Image) -> bytes:
    import io

    buf = io.BytesIO()
    img.save(buf, format="PNG")
    return buf.getvalue()


def write_ico(path: Path, images: dict[int, Image.Image]) -> None:
    sizes = sorted(images)
    payloads = [
        png_payload(images[s]) if s >= 256 else bmp_payload(images[s]) for s in sizes
    ]

    offset = 6 + 16 * len(sizes)
    out = bytearray(struct.pack("<HHH", 0, 1, len(sizes)))  # reserved, type=icon, count
    for size, payload in zip(sizes, payloads):
        out += struct.pack(
            "<BBBBHHII",
            size if size < 256 else 0,  # 0 encodes 256
            size if size < 256 else 0,
            0,  # colour count — 0 for true colour
            0,  # reserved
            1,  # planes
            32,  # bits per pixel
            len(payload),
            offset,
        )
        offset += len(payload)
    for payload in payloads:
        out += payload

    path.write_bytes(bytes(out))


def main() -> None:
    ico_images = {s: render(s) for s in ICO_SIZES}
    write_ico(ICONS / "icon.ico", ico_images)
    print(f"wrote icon.ico — sizes {ICO_SIZES}")

    for s in TRAY_SIZES:
        out = ICONS / f"tray-windows-{s}.png"
        ico_images[s].save(out, format="PNG")
        print(f"wrote {out.name} — {s}x{s}")


if __name__ == "__main__":
    main()
