#!/usr/bin/env python3
"""Generate the Verve app icon (1024x1024 master + all sizes + .icns + .ico).

Design language (macOS Big Sur / Sonoma style):
  • Squircle with rich indigo→violet→blue gradient.
  • Subtle top gloss highlight + soft drop shadow beneath the shape.
  • Bold white "V" chevron (Verve / downward request / check) with crisp
    highlight on top edge and soft contact shadow underneath.
  • Small cyan pulse dot upper-right (API / network / live signal).
"""
import math
import shutil
import subprocess
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageChops

ROOT = Path(__file__).resolve().parent.parent
ICON_DIR = ROOT / "assets" / "icons"
ICON_DIR.mkdir(parents=True, exist_ok=True)

SIZE = 1024


# ---------------------------------------------------------------------------
# Vectorized squircle mask via numpy (superellipse)
# ---------------------------------------------------------------------------
def squircle_mask(size: int, radius: float = 0.225) -> Image.Image:
    """Anti-aliased squircle (superellipse) L-mask."""
    n = 5.2  # exponent — 5 ≈ iOS squircle
    c = size / 2
    yy, xx = np.mgrid[0:size, 0:size].astype(np.float32)
    dx = (xx - c) / c
    dy = (yy - c) / c
    val = (np.abs(dx) ** n + np.abs(dy) ** n) ** (1.0 / n)
    # 1.0 = on the shape boundary; ramp a couple of pixels for AA
    edge = 1.6 / size  # AA band width in normalized coords
    alpha = np.clip((1.0 - (val - (1.0 - edge)) / edge) * 255.0, 0, 255).astype(np.uint8)
    return Image.fromarray(alpha, mode="L")


# ---------------------------------------------------------------------------
# Gradient helpers (numpy-based for smooth blends)
# ---------------------------------------------------------------------------
def vert_gradient(size: int, top_rgb, bot_rgb) -> Image.Image:
    h = w = size
    t = np.linspace(0, 1, h, dtype=np.float32)[:, None, None]
    top = np.array(top_rgb, dtype=np.float32)[None, None, :]
    bot = np.array(bot_rgb, dtype=np.float32)[None, None, :]
    rgb = top * (1 - t) + bot * t
    rgb = np.repeat(rgb, w, axis=1)
    arr = np.dstack([rgb, np.full((h, w, 1), 255, dtype=np.uint8)]).astype(np.uint8)
    return Image.fromarray(arr, "RGBA")


def radial_gradient(size: int, center, radius, inner_rgba, outer_rgba, elliptical=1.0):
    """RGBA radial gradient (numpy). inner/outer = (r,g,b,a)."""
    cx, cy = center
    yy, xx = np.mgrid[0:size, 0:size].astype(np.float32)
    d = np.sqrt(((xx - cx) / elliptical) ** 2 + (yy - cy) ** 2)
    t = np.clip(d / radius, 0.0, 1.0)[..., None]
    inner = np.array(inner_rgba, dtype=np.float32)
    outer = np.array(outer_rgba, dtype=np.float32)
    arr = (inner * (1 - t) + outer * t).astype(np.uint8)
    return Image.fromarray(arr, "RGBA")


# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
def build_master() -> Image.Image:
    s = SIZE
    mask = squircle_mask(s, radius=0.225)

    # 1) Background gradient — deep indigo top → vivid blue bottom, with a
    #    subtle purple/violet mid-band for richness.
    base = vert_gradient(s, (38, 55, 185), (20, 115, 240))
    # Violet glow in upper-middle (subtle, elliptical for a horizontal band).
    mid_glow = radial_gradient(s, (s // 2, int(s * 0.40)), int(s * 0.55),
                               (120, 85, 230, 160), (120, 85, 230, 0), elliptical=1.3)
    base = Image.alpha_composite(base, mid_glow)
    base.putalpha(mask)

    # 2) Outer drop shadow beneath the shape (blur a solid mask, offset down).
    shadow = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    sh_layer = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    # Black alpha mask = squircle shape filled with black; blur just its alpha.
    black = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    black.putalpha(mask)
    # Paint fully opaque black wherever the mask is >0, preserving alpha.
    from PIL import ImageChops as IC
    black = Image.merge("RGBA", (
        Image.new("L", (s, s), 0),
        Image.new("L", (s, s), 0),
        Image.new("L", (s, s), 0),
        black.split()[3],
    ))
    # Extend and blur.
    shadow_alpha = black.split()[3].filter(ImageFilter.GaussianBlur(38))
    shadow = Image.merge("RGBA", (
        Image.new("L", (s, s), 0),
        Image.new("L", (s, s), 0),
        Image.new("L", (s, s), 0),
        shadow_alpha,
    ))
    # Reduce peak alpha so it's a soft shadow, not a black silhouette.
    sa = np.array(shadow.split()[3], dtype=np.float32)
    sa = (sa * 0.55).clip(0, 255).astype(np.uint8)
    shadow = Image.merge("RGBA", (
        Image.new("L", (s, s), 0),
        Image.new("L", (s, s), 0),
        Image.new("L", (s, s), 0),
        Image.fromarray(sa),
    ))
    shadow_canvas = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    shadow_canvas.paste(shadow, (0, 46), shadow)

    out = Image.alpha_composite(shadow_canvas, base)

    # 3) Subtle top gloss — a soft elliptical white sheen peeking from the
    # top edge (think Big Sur Safari / Messages icons). Centered above the
    # canvas so its brightest part is clipped off-canvas.
    yy, xx = np.mgrid[0:s, 0:s].astype(np.float32)
    cy = int(s * -0.25)
    # Distance from center of an ellipse much wider than tall.
    dx = (xx - s / 2) / (s * 0.65)
    dy = (yy - cy) / (s * 0.45)
    d = np.sqrt(dx * dx + dy * dy)
    gloss_alpha = np.clip((1.0 - d), 0, 1) ** 1.8 * 95
    rgb = np.zeros((s, s, 4), dtype=np.uint8)
    rgb[:, :, 0] = 255
    rgb[:, :, 1] = 255
    rgb[:, :, 2] = 255
    rgb[:, :, 3] = gloss_alpha.astype(np.uint8)
    gloss = Image.fromarray(rgb, "RGBA")
    gloss.putalpha(ImageChops.multiply(gloss.split()[3], mask))
    out = Image.alpha_composite(out, gloss)

    # 5) The "V" chevron mark.
    # ---- Build V as a solid white polygon with rounded line, on its own layer.
    stroke = int(s * 0.112)
    left_top = (int(s * 0.26), int(s * 0.30))
    vertex = (int(s * 0.50), int(s * 0.73))
    right_top = (int(s * 0.74), int(s * 0.30))

    r = stroke // 2

    # 5a) Subtle drop shadow under V — tight dark blur, offset slightly down.
    # This makes the white V feel lifted off the background (Sonoma style).
    v_shadow = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    vsd = ImageDraw.Draw(v_shadow)
    vsd.line([left_top, vertex, right_top], fill=(30, 30, 80, 255), width=stroke, joint="curve")
    for pt in (left_top, vertex, right_top):
        vsd.ellipse([pt[0] - r, pt[1] - r, pt[0] + r, pt[1] + r], fill=(30, 30, 80, 255))
    vsa = np.array(v_shadow.split()[3], dtype=np.float32)
    v_shadow = Image.merge("RGBA", (
        Image.new("L", (s, s), 20),
        Image.new("L", (s, s), 20),
        Image.new("L", (s, s), 60),
        Image.fromarray((vsa * 0.75).astype(np.uint8)),
    )).filter(ImageFilter.GaussianBlur(10))
    v_shadow_canvas = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    v_shadow_canvas.paste(v_shadow, (3, 10), v_shadow)
    out = Image.alpha_composite(out, v_shadow_canvas)

    # 5b) The V itself — pure bright solid white, crisp and clean.
    v_layer = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    vd = ImageDraw.Draw(v_layer)
    vd.line([left_top, vertex, right_top], fill=(255, 255, 255, 255), width=stroke, joint="curve")
    for pt in (left_top, vertex, right_top):
        vd.ellipse([pt[0] - r, pt[1] - r, pt[0] + r, pt[1] + r], fill=(255, 255, 255, 255))
    out = Image.alpha_composite(out, v_layer)

    # 6) Bright cyan pulse dot upper-right (live / API signal).
    dot_cx, dot_cy = int(s * 0.80), int(s * 0.22)
    glow_r = int(s * 0.060)
    dot_r = int(s * 0.028)
    # Outer soft cyan glow.
    glow = radial_gradient(s, (dot_cx, dot_cy), glow_r,
                           (110, 225, 255, 180), (110, 225, 255, 0), elliptical=1.0)
    glow.putalpha(ImageChops.multiply(glow.split()[3], mask))
    out = Image.alpha_composite(out, glow)
    # Solid white dot.
    dot = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    dd = ImageDraw.Draw(dot)
    dd.ellipse([dot_cx - dot_r, dot_cy - dot_r, dot_cx + dot_r, dot_cy + dot_r],
               fill=(255, 255, 255, 255))
    dot.putalpha(ImageChops.multiply(dot.split()[3], mask))
    out = Image.alpha_composite(out, dot)

    # 7) Very subtle bottom vignette — darken only the bottom ~15% of the icon
    # (linear vertical fade from full-dark at bottom to zero at y=85%). This
    # grounds the shape without touching the V or dot.
    vign_alpha = np.zeros((s, s), dtype=np.float32)
    fade_start = int(s * 0.85)
    for y in range(fade_start, s):
        t = (y - fade_start) / (s - fade_start)
        vign_alpha[y, :] = t * 55  # peak alpha at bottom edge
    vign = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    # Build RGBA: constant dark color, alpha = the linear fade.
    rgb = np.zeros((s, s, 4), dtype=np.uint8)
    rgb[:, :, 3] = vign_alpha.astype(np.uint8)
    vign = Image.fromarray(rgb, "RGBA")
    vign.putalpha(ImageChops.multiply(vign.split()[3], mask))
    out = Image.alpha_composite(out, vign)

    # Final clip to squircle mask to clean any fringe from blurs.
    out.putalpha(ImageChops.multiply(out.split()[3], mask))
    return out


# ---------------------------------------------------------------------------
# Output helpers
# ---------------------------------------------------------------------------
def make_icns(master: Image.Image, icns_path: Path):
    iconset = icns_path.with_suffix(".iconset")
    if iconset.exists():
        shutil.rmtree(iconset)
    iconset.mkdir(parents=True)
    spec = [
        ("icon_16x16.png", 16),
        ("icon_16x16@2x.png", 32),
        ("icon_32x32.png", 32),
        ("icon_32x32@2x.png", 64),
        ("icon_128x128.png", 128),
        ("icon_128x128@2x.png", 256),
        ("icon_256x256.png", 256),
        ("icon_256x256@2x.png", 512),
        ("icon_512x512.png", 512),
        ("icon_512x512@2x.png", 1024),
    ]
    for name, px in spec:
        master.resize((px, px), Image.LANCZOS).save(iconset / name, "PNG")
    try:
        subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(icns_path)],
                       check=True, capture_output=True)
        print(f"✅ wrote {icns_path}")
    except FileNotFoundError:
        print("⚠️  iconutil unavailable — skipped .icns")
    except subprocess.CalledProcessError as e:
        print(f"❌ iconutil failed: {e.stderr.decode(errors='replace')}")


def make_ico(master: Image.Image, ico_path: Path):
    sizes = [16, 32, 48, 64, 128, 256]
    imgs = [master.resize((s, s), Image.LANCZOS) for s in sizes]
    imgs[0].save(ico_path, format="ICO", sizes=[(s, s) for s in sizes])
    print(f"✅ wrote {ico_path}")


def main():
    print("🎨 Rendering 1024px master icon...")
    master = build_master()

    master_png = ICON_DIR / "icon_1024x1024.png"
    master.save(master_png, "PNG", optimize=True)
    print(f"✅ wrote {master_png}")

    for px in (16, 32, 64, 128, 256, 512):
        p = ICON_DIR / f"icon_{px}x{px}.png"
        master.resize((px, px), Image.LANCZOS).save(p, "PNG", optimize=True)
        print(f"✅ wrote {p}")

    master.resize((512, 512), Image.LANCZOS).save(ICON_DIR / "verve.png", "PNG", optimize=True)
    print(f"✅ wrote {ICON_DIR / 'verve.png'}")

    make_icns(master, ICON_DIR / "verve.icns")
    make_ico(master, ICON_DIR / "verve.ico")

    print("\n🎉 Icon generation complete.")


if __name__ == "__main__":
    main()
