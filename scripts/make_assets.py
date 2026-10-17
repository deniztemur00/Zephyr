import io
import struct
from pathlib import Path

import numpy as np
from PIL import Image, ImageFilter

HERE = Path(__file__).resolve().parent.parent / "assets"
SOURCE = HERE / "shortcut_icon.jpg"
TONE = (239, 230, 207)
ICON_TONE = (0xD9, 0xA9, 0x3A)  # Forest Walk accent: readable on dark and light taskbars
NOISE = 0.05
BREEZE_WIDTH = 360
ICON_SIZES = (16, 24, 32, 48, 64, 128, 256)
ICON_FILL = 0.96
BOLDER = {16: 15, 24: 11, 32: 7, 48: 5}  # thicken strokes before shrinking so small icons stay legible


def top_right_darkness():
    gray = np.asarray(Image.open(SOURCE).convert("L"), dtype=np.float32) / 255.0
    height, width = gray.shape
    dark = 1.0 - gray[: height // 2, width // 2:]
    top = np.percentile(dark[dark > NOISE], 99.5)
    return np.clip((dark - NOISE) / (top - NOISE), 0, 1)


def cropped(dark, padding=4):
    ys, xs = np.nonzero(dark > 0)
    box = (max(xs.min() - padding, 0), max(ys.min() - padding, 0),
           min(xs.max() + padding + 1, dark.shape[1]), min(ys.max() + padding + 1, dark.shape[0]))
    return Image.fromarray((dark * 255).astype(np.uint8)).crop(box)


def tinted(alpha):
    image = Image.new("RGBA", alpha.size, TONE + (255,))
    image.putalpha(alpha)
    return image


def breeze(drawing):
    height = round(drawing.height * BREEZE_WIDTH / drawing.width)
    return tinted(drawing.resize((BREEZE_WIDTH, height), Image.LANCZOS))


def icon(drawing, size):
    bold = drawing.filter(ImageFilter.MaxFilter(BOLDER.get(size, 1))) if size in BOLDER else drawing
    scale = size * ICON_FILL / max(bold.size)
    fitted = bold.resize((max(1, round(bold.width * scale)), max(1, round(bold.height * scale))), Image.LANCZOS)
    glyph = Image.new("RGBA", fitted.size, ICON_TONE + (255,))
    glyph.putalpha(fitted)
    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    canvas.alpha_composite(glyph, ((size - fitted.width) // 2, (size - fitted.height) // 2))
    return canvas


def png_bytes(image):
    buffer = io.BytesIO()
    image.save(buffer, "PNG", optimize=True)
    return buffer.getvalue()


def resource(kind, name, data, flags):
    header = struct.pack("<IIHHHHIHHII", len(data), 32, 0xFFFF, kind, 0xFFFF, name, 0, flags, 0, 0, 0)
    return header + data + b"\0" * (-len(data) % 4)


def res_file(pngs):
    icons = b"".join(resource(3, i + 1, png, 0x1010) for i, png in enumerate(pngs.values()))
    entries = b"".join(struct.pack("<BBBBHHIH", size % 256, size % 256, 0, 0, 1, 32, len(png), i + 1)
                       for i, (size, png) in enumerate(pngs.items()))
    group = struct.pack("<HHH", 0, 1, len(pngs)) + entries
    return resource(0, 0, b"", 0) + icons + resource(14, 1, group, 0x1030)


def main():
    drawing = cropped(top_right_darkness())
    breeze(drawing).save(HERE / "zephyr_breeze.png", optimize=True)
    icons = {size: icon(drawing, size) for size in ICON_SIZES}
    icons[256].save(HERE / "zephyr.ico", sizes=[(s, s) for s in ICON_SIZES], append_images=list(icons.values()))
    icons[64].save(HERE / "zephyr_icon.png", optimize=True)
    (HERE / "zephyr.res").write_bytes(res_file({size: png_bytes(image) for size, image in icons.items()}))
    for name in ("zephyr_breeze.png", "zephyr.ico", "zephyr.res", "zephyr_icon.png"):
        print(f"{name}: {(HERE / name).stat().st_size} bytes")


if __name__ == "__main__":
    main()
