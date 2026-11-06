"""Brand assets with text as outlines: python assets/src/build.py <font dir> [out dir]

The font dir holds the variable fonts MartianMono.ttf and InstrumentSans.ttf (SIL Open Font License)."""

import sys
from pathlib import Path

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

FONTS = Path(sys.argv[1])
OUT = Path(sys.argv[2] if len(sys.argv) > 2 else "assets")
OUT.mkdir(parents=True, exist_ok=True)


def instance(name, **axes):
    return instantiateVariableFont(TTFont(FONTS / name), axes)


MARK = instance("MartianMono.ttf", wght=700, wdth=100)
MONO = instance("MartianMono.ttf", wght=400, wdth=100)
MEDIUM = instance("InstrumentSans.ttf", wght=500, wdth=100)
REGULAR = instance("InstrumentSans.ttf", wght=400, wdth=100)

INK, PAPER, ACCENT = "#121210", "#F5F4EE", "#D4F13A"
THEMES = {
    "light": {"text": INK, "ground": PAPER, "tagline": "#5E5D56"},
    "dark": {"text": PAPER, "ground": INK, "tagline": "#A3A29A"},
}


def text_path(font, text, x, baseline, size, tracking=0.0):
    """SVG path data for `text` and its advance width, in user units."""
    glyphs = font.getGlyphSet()
    cmap = font.getBestCmap()
    scale = size / font["head"].unitsPerEm
    pen = SVGPathPen(glyphs, ntos=lambda v: f"{v:.1f}".rstrip("0").rstrip("."))
    cursor = x
    for ch in text:
        name = cmap[ord(ch)]
        glyphs[name].draw(TransformPen(pen, (scale, 0, 0, -scale, cursor, baseline)))
        cursor += glyphs[name].width * scale + tracking * size
    return pen.getCommands(), cursor - x - tracking * size


def wordmark(x, baseline, size, theme):
    """`comple` as typed, then `type` as the completion, selected like an address bar's inline suggestion."""
    head, head_w = text_path(MARK, "comple", x, baseline, size)
    tail_x = x + head_w
    tail, tail_w = text_path(MARK, "type", tail_x, baseline, size)
    top, bottom = baseline - size * 0.86, baseline + size * 0.3
    caret = size * 0.07
    parts = [
        f'<rect x="{tail_x:.2f}" y="{top:.2f}" width="{tail_w + size * 0.06:.2f}" height="{bottom - top:.2f}" fill="{ACCENT}"/>',
        f'<path d="{head}" fill="{THEMES[theme]["text"]}"/>',
        f'<path d="{tail}" fill="{INK}"/>',
        f'<rect x="{tail_x - caret / 2:.2f}" y="{top - size * 0.08:.2f}" width="{caret:.2f}" '
        f'height="{bottom - top + size * 0.16:.2f}" fill="{THEMES[theme]["text"]}"/>',
    ]
    return "\n".join(parts), tail_w + head_w + size * 0.06


def svg(width, height, body, label):
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" width="{width}" height="{height}" '
        f'role="img" aria-label="{label}">\n{body}\n</svg>\n'
    )


def logo():
    """A `c` and the block of its completion on an ink tile."""
    size = 92
    c, c_w = text_path(MARK, "c", 0, 0, size)
    block_w, gap = size * 0.34, size * 0.04
    left = (128 - (c_w + gap + block_w)) / 2
    baseline = 64 + size * 0.3
    c, _ = text_path(MARK, "c", left, baseline, size)
    top = baseline - size * 0.6
    return (
        f'<rect width="128" height="128" rx="28" fill="{INK}"/>\n<path d="{c}" fill="{PAPER}"/>\n'
        f'<rect x="{left + c_w + gap:.2f}" y="{top:.2f}" width="{block_w:.2f}" height="{baseline - top:.2f}" fill="{ACCENT}"/>'
    )


TAGLINE = "Serverless autocompletion for Rust and Python"
KINDS = ["exact", "prefix", "infix", "abbreviation", "spelling", "decomposition", "semantic"]

if __name__ == "__main__":
    (OUT / "logo.svg").write_text(svg(128, 128, logo(), "completype"))

    for theme, colours in THEMES.items():
        mark, mark_w = wordmark(28, 96, 64, theme)
        tag, tag_w = text_path(MEDIUM, TAGLINE, 30, 150, 24)
        width = int(max(28 + mark_w, 30 + tag_w) + 28)
        # A background of its own keeps the banner legible where the page's theme differs from it.
        body = f'<rect width="{width}" height="184" rx="16" fill="{colours["ground"]}"/>\n{mark}\n<path d="{tag}" fill="{colours["tagline"]}"/>'
        (OUT / f"banner-{theme}.svg").write_text(svg(width, 184, body, f"completype: {TAGLINE.lower()}"))

        size, pad = 64, 4
        baseline = pad + size * 0.94
        mark, mark_w = wordmark(pad + size * 0.04, baseline, size, theme)
        height = int(baseline + size * 0.38 + pad)
        (OUT / f"wordmark-{theme}.svg").write_text(svg(int(mark_w + size * 0.08 + 2 * pad), height, mark, "completype"))

    mark, mark_w = wordmark(0, 0, 112, "dark")
    tag, tag_w = text_path(MEDIUM, TAGLINE, 0, 0, 40)
    kinds, cursor = [], 0.0
    for i, kind in enumerate(KINDS):
        if i:
            kinds.append(f'<rect x="{cursor - 16:.1f}" y="-10" width="6" height="6" fill="{ACCENT}"/>')
        word, word_w = text_path(MONO, kind, cursor, 0, 22)
        kinds.append(f'<path d="{word}" fill="#A3A29A"/>')
        cursor += word_w + 22 * 1.2
    left = (1280 - max(mark_w, tag_w, cursor)) / 2
    social = f"""<rect width="1280" height="640" fill="{INK}"/>
<g transform="translate({left:.1f} 300)">{mark}</g>
<g transform="translate({left + 2:.1f} 408)"><path d="{tag}" fill="#A3A29A"/></g>
<rect x="{left + 2:.1f}" y="462" width="{cursor - 26.4:.1f}" height="1" fill="#34332E"/>
<g transform="translate({left + 2:.1f} 506)">{"".join(kinds)}</g>"""
    (OUT / "social-preview.svg").write_text(svg(1280, 640, social, "completype"))
    try:
        import cairosvg

        cairosvg.svg2png(url=str(OUT / "social-preview.svg"), write_to=str(OUT / "social-preview.png"))
    except ImportError:
        print("cairosvg is not installed; social-preview.png not refreshed")
    # The docs site serves its own copies.
    DOCS = Path("docs/assets")
    if DOCS.is_dir():
        for name in ("logo.svg", "wordmark-light.svg", "wordmark-dark.svg", "banner-light.svg", "banner-dark.svg", "social-preview.png"):
            if (OUT / name).exists():
                (DOCS / name).write_bytes((OUT / name).read_bytes())
    print("written to", OUT)
