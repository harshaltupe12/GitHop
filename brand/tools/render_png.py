"""Render brand SVGs to PNG with headless Chromium (Playwright build; override with GITHOP_CHROME), then build ICO files with Pillow."""
import os, re, subprocess, sys, tempfile
from PIL import Image

ROOT = sys.argv[1]
BRAND = os.path.join(ROOT, "brand")
EDGE = os.environ.get("GITHOP_CHROME") or os.path.expandvars(r"%LOCALAPPDATA%\ms-playwright\chromium_headless_shell-1243\chrome-headless-shell-win64\chrome-headless-shell.exe")
TMP = tempfile.mkdtemp(prefix="githop-render-")


def render(src_rel, out_rel, w, h):
    src = open(os.path.join(BRAND, src_rel), encoding="utf-8").read()
    src = re.sub(r'width="[\d.]+" height="[\d.]+"', f'width="{w}" height="{h}"', src, count=1)
    tmp_svg = os.path.join(TMP, "in.svg")
    open(tmp_svg, "w", encoding="utf-8").write(src)
    out = os.path.join(BRAND, out_rel)
    os.makedirs(os.path.dirname(out), exist_ok=True)
    subprocess.run([EDGE, "--disable-gpu", "--hide-scrollbars", "--default-background-color=00000000",
                    "--force-device-scale-factor=1", f"--window-size={w},{h}",
                    f"--screenshot={out}", "file:///" + tmp_svg.replace("\\", "/")],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=60)
    im = Image.open(out)
    if im.size != (w, h):
        im.crop((0, 0, w, h)).save(out)
    return out


if __name__ == "__main__":
    only = sys.argv[2] if len(sys.argv) > 2 else None
    jobs = []
    for s in (16, 20, 24, 32, 40, 48, 64, 128, 256, 512, 1024):
        src = "svg/githop-favicon.svg" if s <= 32 else "svg/githop-icon-light.svg"
        jobs.append((src, f"png/icon-light/githop-icon-light-{s}.png", s, s))
    for v in ("dark", "aqua"):
        for s in (32, 64, 128, 256, 512, 1024):
            jobs.append((f"svg/githop-icon-{v}.svg", f"png/icon-{v}/githop-icon-{v}-{s}.png", s, s))
    for m in ("color", "reverse", "mono-ink", "mono-white", "mono-aqua"):
        for s in (256, 512):
            jobs.append((f"svg/githop-mark-{m}.svg", f"png/mark/githop-mark-{m}-{s}.png", s, s))
    for l, (w, h) in {"lockup-horizontal": (196, 64), "lockup-horizontal-reverse": (196, 64),
                      "lockup-stacked": (160, 150), "lockup-stacked-reverse": (160, 150),
                      "wordmark": (168, 64), "wordmark-reverse": (168, 64)}.items():
        jobs.append((f"svg/githop-{l}.svg", f"png/lockup/githop-{l}@2x.png", w * 2, h * 2))
        jobs.append((f"svg/githop-{l}.svg", f"png/lockup/githop-{l}@4x.png", w * 4, h * 4))
    jobs += [("graphics/githop-social-preview.svg", "png/graphics/githop-social-preview-1280x640.png", 1280, 640),
             ("graphics/githop-readme-banner.svg", "png/graphics/githop-readme-banner-1600x400.png", 1600, 400),
             ("graphics/githop-readme-banner-dark.svg", "png/graphics/githop-readme-banner-dark-1600x400.png", 1600, 400),
             ("graphics/githop-construction.svg", "png/graphics/githop-construction-1024.png", 1024, 1024)]
    if only:
        jobs = [j for j in jobs if only in j[1]]
    for j in jobs:
        render(*j)
        print("ok", j[1])

    if not only:
        # ICO: each size rendered natively (favicon geometry for <=32 px)
        sizes = (16, 20, 24, 32, 40, 48, 64, 256)
        imgs = [Image.open(os.path.join(BRAND, f"png/icon-light/githop-icon-light-{s}.png")).convert("RGBA") for s in sizes]
        os.makedirs(os.path.join(BRAND, "ico"), exist_ok=True)
        big = imgs[-1]
        big.save(os.path.join(BRAND, "ico/githop.ico"), format="ICO", sizes=[(s, s) for s in sizes], append_images=imgs[:-1])
        fav = [Image.open(os.path.join(BRAND, f"png/icon-light/githop-icon-light-{s}.png")).convert("RGBA") for s in (16, 32, 48)]
        fav[-1].save(os.path.join(BRAND, "ico/favicon.ico"), format="ICO", sizes=[(16, 16), (32, 32), (48, 48)], append_images=fav[:-1])
        print("ico done")
