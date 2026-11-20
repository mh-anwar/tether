"""Tether logo concepts: geometry in shapely (fills only, so the symbolic icons stay valid for GTK),
written out as SVG and rendered with cairosvg."""
import math, sys, io, os
from shapely.geometry import box, LineString, Point, Polygon, MultiPolygon
from shapely.ops import unary_union
from shapely import affinity
import cairosvg
from PIL import Image, ImageDraw

OUT = sys.argv[1] if len(sys.argv) > 1 else "out"
os.makedirs(OUT, exist_ok=True)


def rrect(x0, y0, x1, y1, r):
    if r <= 0:
        return box(x0, y0, x1, y1)
    return box(x0 + r, y0 + r, x1 - r, y1 - r).buffer(r, quad_segs=24)


def ring(x0, y0, x1, y1, r, w):
    return rrect(x0, y0, x1, y1, r).difference(rrect(x0 + w, y0 + w, x1 - w, y1 - w, max(r - w, 0.3)))


def cord(pts, w):
    return LineString(pts).buffer(w / 2, quad_segs=24)


def bezier(p0, p1, p2, p3, n=80):
    out = []
    for i in range(n + 1):
        t = i / n
        a = (1 - t) ** 3; b = 3 * (1 - t) ** 2 * t; c = 3 * (1 - t) * t * t; d = t ** 3
        out.append((a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0], a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1]))
    return out


def path(geom, nd=2):
    polys = [geom] if isinstance(geom, Polygon) else list(getattr(geom, "geoms", []))
    parts = []
    for p in polys:
        if p.is_empty:
            continue
        for r in [p.exterior, *p.interiors]:
            cs = list(r.coords)[:-1]
            parts.append("M" + "L".join(f"{round(x, nd):g} {round(y, nd):g}" for x, y in cs) + "Z")
    return "".join(parts)


# ---------- concepts: each returns [(geometry, role)] on a 16-unit grid ----------
# roles: "a" = main (white on the app icon), "b" = second device (tinted on the app icon)
GAP = 1.0


def links():
    """A phone and a desktop screen, interlocked like two links of a chain."""
    phone = ring(1, 1, 9, 15, 2.6, 2)
    screen = ring(4, 4, 15, 12, 2.6, 2)
    # Top crossing: the phone passes over the screen. Bottom crossing: the screen passes over.
    phone_right = box(7, 1, 9, 15)
    screen_bottom = box(4, 10, 15, 12)
    screen = screen.difference(phone_right.intersection(box(0, 0, 16, 8)).buffer(GAP, join_style=2))
    phone = phone.difference(screen_bottom.intersection(box(6, 8, 10, 16)).buffer(GAP, join_style=2))
    return [(phone, "a"), (screen, "b")]


def ellipse(cx, cy, rx, ry, rot):
    e = affinity.scale(Point(0, 0).buffer(1, quad_segs=48), rx, ry)
    return affinity.translate(affinity.rotate(e, rot, origin=(0, 0)), cx, cy)


def orbit():
    """A phone with a tether looped around it: the band passes in front low and behind high."""
    phone = rrect(5.2, 1, 10.8, 15, 1.4)
    band = ellipse(8, 8.4, 7.5, 3.3, -28).difference(ellipse(8, 8.4, 5.5, 1.3, -28))
    front = band.intersection(affinity.rotate(box(-8, 8.4, 24, 24), -28, origin=(8, 8.4)))
    back = band.difference(front)
    back = back.difference(phone.buffer(GAP))
    phone = phone.difference(front.buffer(GAP))
    return [(phone, "a"), (unary_union([front, back]), "b")]


def leash():
    """A phone on a cord whose far end is the computer."""
    phone = ring(1.5, 1, 8.5, 12, 2.2, 1.9)
    pts = bezier((5, 11.5), (5, 15.8), (12.4, 15.8), (12.4, 10.6))
    c = cord(pts, 1.9)
    dot = Point(12.4, 6.6).buffer(2.8, quad_segs=32)
    body = unary_union([phone, c]).difference(dot.buffer(0.7))
    return [(body, "a"), (dot, "b")]


CONCEPTS = {
    "links": (links, ("#3d7cf5", "#1f4fd0"), "#ffffff", "#bcd3ff"),
    "orbit": (orbit, ("#18b199", "#0b7f73"), "#ffffff", "#c4f2e8"),
    "leash": (leash, ("#6a55e8", "#4632c2"), "#ffffff", "#ffc7a8"),
}


def app_svg(name, mark_scale=21.0, pad=24, radius=104, size=512, small=False):
    if small:
        mark_scale, pad, radius = 26.0, 0, 96
    fn, (top, bot), ca, cb = CONCEPTS[name]
    off = (size - 16 * mark_scale) / 2
    shapes = []
    for g, role in fn():
        g = affinity.affine_transform(g, [mark_scale, 0, 0, mark_scale, off, off])
        shapes.append(f'<path fill="{ca if role == "a" else cb}" fill-rule="evenodd" d="{path(g, 1)}"/>')
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" viewBox="0 0 {size} {size}">
<defs><linearGradient id="bg" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{top}"/><stop offset="1" stop-color="{bot}"/></linearGradient></defs>
<rect x="{pad}" y="{pad}" width="{size - 2 * pad}" height="{size - 2 * pad}" rx="{radius}" fill="url(#bg)"/>
{chr(10).join(shapes)}
</svg>
'''


def symbolic_svg(name):
    fn = CONCEPTS[name][0]
    geom = unary_union([g for g, _ in fn()])
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16">
<style id="current-color-scheme">.ColorScheme-Text {{ color:#232629; }}</style>
<path class="ColorScheme-Text" fill="currentColor" fill-rule="evenodd" d="{path(geom, 2)}"/>
</svg>
'''


def png(svg, px, colour=None):
    if colour:
        svg = svg.replace("#232629", colour)
    return Image.open(io.BytesIO(cairosvg.svg2png(bytestring=svg.encode(), output_width=px, output_height=px))).convert("RGBA")


SIZES = [512, 128, 32, 16]
if __name__ == "__main__":
    for n in CONCEPTS:
        open(f"{OUT}/{n}.svg", "w").write(app_svg(n))
        open(f"{OUT}/{n}-symbolic.svg", "w").write(symbolic_svg(n))
        for s in SIZES:
            png(app_svg(n), s).save(f"{OUT}/{n}-{s}.png")
    # Comparison sheet: per concept a row on light and a row on dark; app icon at every size,
    # then the symbolic icon at 16 and 32 in the panel's text colour. Small sizes also shown
    # enlarged 8x (nearest neighbour) so the real pixels can be judged.
    rows = []
    W = 2000
    for n in CONCEPTS:
        for bg, fg in (("#f6f5f4", "#232629"), ("#242424", "#eeeeee")):
            row = Image.new("RGBA", (W, 560), bg)
            x = 20
            for s in SIZES:
                im = png(app_svg(n), s)
                row.alpha_composite(im, (x, 280 - s // 2)); x += s + 30
            for s in (16, 32):
                im = png(app_svg(n, small=True), s).resize((s * 8, s * 8), Image.NEAREST)
                row.alpha_composite(im, (x, 280 - s * 4)); x += s * 8 + 30
            x += 20
            for s in (16, 32):
                im = png(symbolic_svg(n), s, fg)
                row.alpha_composite(im, (x, 280 - s // 2)); x += s + 20
            im = png(symbolic_svg(n), 16, fg).resize((128, 128), Image.NEAREST)
            row.alpha_composite(im, (x, 216)); x += 150
            ImageDraw.Draw(row).text((20, 10), n, fill=fg)
            rows.append(row)
    sheet = Image.new("RGBA", (W, 560 * len(rows)))
    for i, r in enumerate(rows):
        sheet.paste(r, (0, 560 * i))
    sheet.convert("RGB").save(f"{OUT}/sheet.png")
    print("ok", sheet.size)
