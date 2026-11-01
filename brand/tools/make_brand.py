"""Generate the GitHop brand kit (SVG sources) into <repo>/brand/."""
import base64, os, sys

ROOT = sys.argv[1]
OUT = os.path.join(ROOT, "brand")
FONT = os.path.join(ROOT, "app/ui/node_modules/@fontsource/ibm-plex-sans/files/ibm-plex-sans-latin-600-normal.woff2")

INK, AQUA, WHITE, CLEAN, BG, MUTED = "#0E3B35", "#4ABDAC", "#FFFFFF", "#DFDCE3", "#F4F3F6", "#6E6A74"
TEXT = "#1F1D24"

LOOP = "M12 49 C24 49 38 46 36 33 C35 25 24 25 25 33 C26 40 33 41 38 36 L46.5 27.5"
HEAD = "54,20 51.2,31.2 42.8,22.8"


def mark(loop, head, sw=5.5):
    return (f'<g transform="translate(-1 -2.5)">'
            f'<path d="{LOOP}" fill="none" stroke="{loop}" stroke-width="{sw}" stroke-linecap="round" stroke-linejoin="round"/>'
            f'<polygon points="{HEAD}" fill="{head}" stroke="{head}" stroke-width="2" stroke-linejoin="round"/></g>')


def tile(fill, border=None):
    t = f'<rect width="64" height="64" rx="15" fill="{fill}"/>'
    if border:
        t += f'<rect x="0.5" y="0.5" width="63" height="63" rx="14.5" fill="none" stroke="{border}"/>'
    return t


ICONS = {
    "icon-light": tile(WHITE, CLEAN) + mark(INK, AQUA),
    "icon-dark": tile(INK) + mark(WHITE, AQUA),
    "icon-aqua": tile(AQUA) + mark(INK, WHITE),
    "favicon": tile(WHITE, CLEAN) + mark(INK, AQUA, sw=7),
}
MARKS = {
    "mark-color": mark(INK, AQUA),
    "mark-reverse": mark(WHITE, AQUA),
    "mark-mono-ink": mark(INK, INK),
    "mark-mono-white": mark(WHITE, WHITE),
    "mark-mono-aqua": mark(AQUA, AQUA),
}


def svg(w, h, body, title, vb=None, defs=""):
    vb = vb or f"0 0 {w} {h}"
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="{vb}" role="img" aria-label="{title}">'
            f'<title>{title}</title>{defs}{body}</svg>\n')


font_b64 = base64.b64encode(open(FONT, "rb").read()).decode()
FONT_DEFS = ('<defs><style>@font-face{font-family:"GitHop Plex";font-weight:600;'
             f'src:url(data:font/woff2;base64,{font_b64}) format("woff2");}}'
             '.wm{font-family:"GitHop Plex","IBM Plex Sans","Segoe UI",sans-serif;font-weight:600;letter-spacing:-0.02em}'
             '.tg{font-family:"GitHop Plex","IBM Plex Sans","Segoe UI",sans-serif;font-weight:600}</style></defs>')


def icon_at(x, y, size, key):
    return f'<g transform="translate({x} {y}) scale({size / 64})">{ICONS[key]}</g>'


def mark_at(x, y, size, key):
    return f'<g transform="translate({x} {y}) scale({size / 64})">{MARKS[key]}</g>'


def write(rel, content):
    p = os.path.join(OUT, rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "w", encoding="utf-8", newline="\n") as f:
        f.write(content)


# Icons and marks
for k, b in ICONS.items():
    write(f"svg/githop-{k}.svg", svg(64, 64, b, "GitHop"))
for k, b in MARKS.items():
    write(f"svg/githop-{k}.svg", svg(64, 64, b, "GitHop"))

# Wordmark (font embedded). IBM Plex Sans 600 at 48px: "GitHop" ≈ 152px wide.
for name, color in (("wordmark", TEXT), ("wordmark-reverse", WHITE)):
    write(f"svg/githop-{name}.svg",
          svg(168, 64, f'<text class="wm" x="4" y="47" font-size="48" fill="{color}">GitHop</text>', "GitHop", defs=FONT_DEFS))

# Lockups
def lockup_h(icon, color):
    return (icon_at(0, 0, 64, icon) +
            f'<text class="wm" x="82" y="44" font-size="34" fill="{color}">GitHop</text>')

write("svg/githop-lockup-horizontal.svg", svg(196, 64, lockup_h("icon-light", TEXT), "GitHop", defs=FONT_DEFS))
write("svg/githop-lockup-horizontal-reverse.svg", svg(196, 64, lockup_h("icon-dark", WHITE), "GitHop", defs=FONT_DEFS))
write("svg/githop-lockup-stacked.svg",
      svg(160, 150, icon_at(40, 0, 80, "icon-light") +
          f'<text class="wm" x="80" y="138" font-size="34" fill="{TEXT}" text-anchor="middle">GitHop</text>', "GitHop", defs=FONT_DEFS))
write("svg/githop-lockup-stacked-reverse.svg",
      svg(160, 150, icon_at(40, 0, 80, "icon-dark") +
          f'<text class="wm" x="80" y="138" font-size="34" fill="{WHITE}" text-anchor="middle">GitHop</text>', "GitHop", defs=FONT_DEFS))

# Graphics
TAG = "One PC. Every git identity. No mix-ups."
social = (f'<rect width="1280" height="640" fill="{BG}"/>' +
          mark_at(820, 150, 560, "mark-mono-aqua").replace("<g transform", '<g opacity="0.10" transform', 1) +
          icon_at(120, 190, 140, "icon-light") +
          f'<text class="wm" x="292" y="292" font-size="104" fill="{TEXT}">GitHop</text>'
          f'<text class="tg" x="124" y="420" font-size="40" fill="{TEXT}">{TAG}</text>'
          f'<text class="tg" x="124" y="476" font-size="26" fill="{MUTED}">Local-only git identity switcher for Windows</text>'
          f'<rect x="124" y="522" width="96" height="8" rx="4" fill="{AQUA}"/>')
write("graphics/githop-social-preview.svg", svg(1280, 640, social, "GitHop — " + TAG, defs=FONT_DEFS))

banner = (f'<rect width="1600" height="400" fill="{BG}"/>' +
          mark_at(1180, -40, 480, "mark-mono-aqua").replace("<g transform", '<g opacity="0.10" transform', 1) +
          icon_at(120, 120, 160, "icon-light") +
          f'<text class="wm" x="320" y="222" font-size="112" fill="{TEXT}">GitHop</text>'
          f'<text class="tg" x="326" y="286" font-size="34" fill="{MUTED}">{TAG}</text>')
write("graphics/githop-readme-banner.svg", svg(1600, 400, banner, "GitHop", defs=FONT_DEFS))

banner_dark = (f'<rect width="1600" height="400" fill="{INK}"/>' +
               mark_at(1180, -40, 480, "mark-mono-aqua").replace("<g transform", '<g opacity="0.14" transform', 1) +
               icon_at(120, 120, 160, "icon-dark").replace(f'fill="{INK}"', 'fill="#0A2C28"', 1) +
               f'<text class="wm" x="320" y="222" font-size="112" fill="{WHITE}">GitHop</text>'
               f'<text class="tg" x="326" y="286" font-size="34" fill="#9BE3D7">{TAG}</text>')
write("graphics/githop-readme-banner-dark.svg", svg(1600, 400, banner_dark, "GitHop", defs=FONT_DEFS))

# Clear-space / construction sheet
grid = "".join(f'<line x1="{i}" y1="0" x2="{i}" y2="64" stroke="#DFDCE3" stroke-width="0.15"/>'
               f'<line x1="0" y1="{i}" x2="64" y2="{i}" stroke="#DFDCE3" stroke-width="0.15"/>' for i in range(0, 65, 4))
construction = (f'<rect width="64" height="64" fill="{WHITE}"/>{grid}' + mark(INK, AQUA) +
                '<rect x="0.25" y="0.25" width="63.5" height="63.5" rx="15" fill="none" stroke="#C2380F" stroke-width="0.4" stroke-dasharray="1 1"/>'
                '<rect x="8" y="16" width="46.5" height="33.5" fill="none" stroke="#4ABDAC" stroke-width="0.35" stroke-dasharray="1 1"/>')
write("graphics/githop-construction.svg", svg(512, 512, construction, "GitHop construction grid", vb="0 0 64 64"))
print("svg done")
