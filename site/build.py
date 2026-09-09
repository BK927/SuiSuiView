"""Build and validate the public introduction site using Python's standard library."""

import argparse
import html
from html.parser import HTMLParser
import json
from pathlib import Path
import shutil
import stat
import struct
from string import Template
from urllib.parse import unquote, urlsplit
import xml.etree.ElementTree as ET


SITE = Path(__file__).resolve().parent
ROOT = SITE.parent
OUTPUT = SITE / "_build"
BASE_URL = "https://bk927.github.io/SuiSuiView/"
BASE_PATH = "/SuiSuiView/"
GITHUB_URL = "https://github.com/BK927/SuiSuiView"
LANGUAGES = {"en": "English", "ko": "한국어", "ja": "日本語", "zh-hans": "简体中文"}
SCREENSHOTS = (
    "reading-spread", "reading-vertical", "reading-bookmarks",
    "upscale-bilinear", "upscale-anime4k", "upscale-cunny", "custom-controls",
    "custom-mouse", "custom-toolbar",
)
ASSETS = {f"{name}.png": ROOT / "assets" / "site" / f"{name}.png" for name in SCREENSHOTS}
ASSETS["icon.png"] = ROOT / "assets" / "site" / "icon.png"


def route(language):
    return "" if language == "en" else language + "/"


def escape(value):
    return html.escape(str(value), quote=True)


def png_dimensions(path):
    with path.open("rb") as file:
        header = file.read(24)
    if len(header) != 24 or header[:8] != b"\x89PNG\r\n\x1a\n" or header[12:16] != b"IHDR":
        raise ValueError(f"Expected a PNG image: {path}")
    width, height = struct.unpack(">II", header[16:24])
    if width == 0 or height == 0:
        raise ValueError(f"Empty PNG image: {path}")
    return width, height


def shape(value):
    """Keep every locale's public content and section structure in sync."""
    if isinstance(value, dict):
        return {key: shape(item) for key, item in value.items()}
    if isinstance(value, list):
        return [shape(item) for item in value]
    if not isinstance(value, str) or not value.strip():
        raise ValueError("Locale copy must contain non-empty strings")
    return "text"


def load_locales():
    locales = {
        language: json.loads((SITE / "locales" / f"{language}.json").read_text(encoding="utf-8"))
        for language in LANGUAGES
    }
    reference = shape(locales["en"])
    for language, data in locales.items():
        if shape(data) != reference:
            raise ValueError(f"Locale structure differs from English: {language}")
    return locales


def cards(items, css_class="feature-card"):
    return "\n".join(
        f'<article class="{css_class}"><h3>{escape(title)}</h3><p>{escape(body)}</p></article>'
        for title, body in items
    )


def figure(name, alt, caption, dimensions, *, hero=False):
    width, height = dimensions[name + ".png"]
    source = BASE_PATH + "assets/" + name + ".png"
    loading = 'fetchpriority="high"' if hero else 'loading="lazy"'
    return (
        f'<figure class="screenshot"><a href="{source}" aria-label="{escape(alt)}">'
        f'<img src="{source}" alt="{escape(alt)}" width="{width}" height="{height}" '
        f'{loading} decoding="async"></a><figcaption>{escape(caption)}</figcaption></figure>'
    )


def render_page(language, data, template, dimensions):
    values = {key: escape(value) for key, value in data.items() if isinstance(value, str)}
    values.update(
        lang=language,
        base=BASE_PATH,
        canonical=BASE_URL + route(language),
        github=GITHUB_URL,
        icon_width=dimensions["icon.png"][0],
        icon_height=dimensions["icon.png"][1],
        alternates="\n".join(
            f'<link rel="alternate" hreflang="{lang}" href="{BASE_URL}{route(lang)}">'
            for lang in LANGUAGES
        ) + f'\n<link rel="alternate" hreflang="x-default" href="{BASE_URL}">',
        languages="\n".join(
            f'<a href="{BASE_PATH}{route(lang)}" lang="{lang}" hreflang="{lang}"'
            + (' aria-current="page"' if lang == language else "")
            + f'>{label}</a>'
            for lang, label in LANGUAGES.items()
        ),
        quality_cards=cards(data["quality_cards"]),
        control_cards=cards(data["control_cards"]),
        reading_cards=cards(data["reading_cards"]),
        format_rows="\n".join(
            f'<tr><th scope="row">{escape(label)}</th><td>{escape(text)}</td></tr>'
            for label, text in data["formats"]
        ),
        build_steps="\n".join(f"<li>{escape(step)}</li>" for step in data["build_steps"]),
        faq="\n".join(
            f'<details><summary>{escape(question)}</summary><p>{escape(answer)}</p></details>'
            for question, answer in data["faq"]
        ),
    )
    for name in SCREENSHOTS:
        alt, caption = data["screens"][name]
        values[name.replace("-", "_")] = figure(
            name, alt, caption, dimensions, hero=name == "reading-spread"
        )
    return template.substitute(values)


def expected_files():
    return {
        *(route(language) + "index.html" for language in LANGUAGES),
        *("assets/" + name for name in ASSETS),
        "assets/style.css", "sitemap.xml", ".nojekyll",
    }


class Page(HTMLParser):
    def __init__(self, content):
        super().__init__(convert_charrefs=True)
        self.elements = []
        self.ids = set()
        self.duplicate_ids = set()
        self.headings = []
        self.feed(content)

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        self.elements.append((tag, attrs))
        if "id" in attrs:
            if attrs["id"] in self.ids:
                self.duplicate_ids.add(attrs["id"])
            self.ids.add(attrs["id"])
        if tag in ("h1", "h2", "h3"):
            self.headings.append(int(tag[1]))

    def attrs(self, tag):
        return [attrs for name, attrs in self.elements if name == tag]


def reject_link(path):
    """lstat also exposes Windows junctions on the supported Python 3.10+."""
    try:
        info = path.lstat()
    except FileNotFoundError:
        return
    if stat.S_ISLNK(info.st_mode) or getattr(info, "st_file_attributes", 0) & 0x400:
        raise ValueError(f"Output must not use symlinks or Windows reparse points: {path}")


def output_files(output):
    for parent in (output, *output.parents):
        reject_link(parent)
    if not output.exists():
        return set()
    files = set()
    allowed_dirs = {"assets", *(language for language in LANGUAGES if language != "en")}
    pending = [output]
    while pending:
        for path in pending.pop().iterdir():
            reject_link(path)
            relative = path.relative_to(output).as_posix()
            if path.is_dir():
                if relative not in allowed_dirs:
                    raise ValueError(f"Unexpected output directory: {relative}")
                pending.append(path)
            elif path.is_file():
                files.add(relative)
            else:
                raise ValueError(f"Unexpected output entry: {relative}")
    return files


def validate(output=OUTPUT):
    actual = output_files(output)
    if actual != expected_files():
        raise ValueError(f"Deployment file mismatch; extra={actual - expected_files()}, missing={expected_files() - actual}")
    pages = {}
    for language in LANGUAGES:
        path = route(language) + "index.html"
        content = (output / path).read_text(encoding="utf-8")
        page = pages[path] = Page(content)
        if page.attrs("html") != [{"lang": language}]:
            raise ValueError(f"Incorrect document language: {path}")
        if page.headings.count(1) != 1 or page.duplicate_ids:
            raise ValueError(f"Heading or duplicate ID error: {path}")
        if any(b > a + 1 for a, b in zip(page.headings, page.headings[1:])):
            raise ValueError(f"Skipped heading level: {path}")
        if "${" in content or "<title></title>" in content or not page.attrs("title"):
            raise ValueError(f"Unrendered template or missing title: {path}")
        meta = {attrs.get("name", attrs.get("property")): attrs.get("content") for attrs in page.attrs("meta")}
        if not meta.get("description") or not meta.get("viewport") or meta.get("og:url") != BASE_URL + route(language):
            raise ValueError(f"Missing or incorrect metadata: {path}")
        canonical = [attrs.get("href") for attrs in page.attrs("link") if attrs.get("rel") == "canonical"]
        if canonical != [BASE_URL + route(language)]:
            raise ValueError(f"Incorrect canonical URL: {path}")
        alternates = {attrs.get("hreflang"): attrs.get("href") for attrs in page.attrs("link") if attrs.get("rel") == "alternate"}
        expected = {lang: BASE_URL + route(lang) for lang in LANGUAGES}
        expected["x-default"] = BASE_URL
        if alternates != expected:
            raise ValueError(f"Incomplete language alternates: {path}")
        for tag, attrs in page.elements:
            if tag in ("script", "iframe", "object", "embed") or any(key.startswith("on") for key in attrs):
                raise ValueError(f"Unexpected active content: {path}")
            if tag == "img":
                image_path = output / unquote(attrs["src"]).removeprefix(BASE_PATH)
                width, height = png_dimensions(image_path)
                decorative_icon = attrs["src"] == BASE_PATH + "assets/icon.png" and attrs.get("alt") == ""
                if (not attrs.get("alt", "").strip() and not decorative_icon) or (attrs.get("width"), attrs.get("height")) != (str(width), str(height)):
                    raise ValueError(f"Missing image text or incorrect dimensions: {path}")
            for key in ("href", "src"):
                if key not in attrs:
                    continue
                url = urlsplit(attrs[key])
                if url.scheme or url.netloc:
                    if url.scheme != "https":
                        raise ValueError(f"Unexpected URL scheme: {attrs[key]}")
                    if key == "src" or (tag == "link" and attrs.get("rel") == "stylesheet"):
                        raise ValueError(f"External page dependency: {attrs[key]}")
                    continue
                if url.path and not url.path.startswith(BASE_PATH):
                    raise ValueError(f"Link escapes project base: {attrs[key]}")
    for path, page in pages.items():
        for _, attrs in page.elements:
            for key in ("href", "src"):
                url = urlsplit(attrs.get(key, ""))
                if not attrs.get(key) or url.scheme or url.netloc:
                    continue
                target = unquote(url.path).removeprefix(BASE_PATH) if url.path else path
                if not target or target.endswith("/"):
                    target += "index.html"
                if target not in actual:
                    raise ValueError(f"Broken local link in {path}: {attrs[key]}")
                if url.fragment and (target not in pages or unquote(url.fragment) not in pages[target].ids):
                    raise ValueError(f"Broken anchor in {path}: {attrs[key]}")
    sitemap = ET.parse(output / "sitemap.xml")
    urls = {node.text for node in sitemap.findall(".//{http://www.sitemaps.org/schemas/sitemap/0.9}loc")}
    if urls != {BASE_URL + route(language) for language in LANGUAGES}:
        raise ValueError("Sitemap does not match the public language pages")


def build(output=OUTPUT, assets=None):
    assets = ASSETS if assets is None else assets
    unexpected = output_files(output) - expected_files()
    if unexpected:
        raise ValueError(f"Refusing to overwrite output with unexpected files: {unexpected}")
    locales = load_locales()
    dimensions = {name: png_dimensions(path) for name, path in assets.items()}
    template = Template((SITE / "template.html").read_text(encoding="utf-8"))
    rendered = {language: render_page(language, data, template, dimensions) for language, data in locales.items()}
    (output / "assets").mkdir(parents=True, exist_ok=True)
    for language, content in rendered.items():
        page = output / route(language) / "index.html"
        page.parent.mkdir(parents=True, exist_ok=True)
        page.write_text(content, encoding="utf-8", newline="\n")
    for name, source in assets.items():
        shutil.copyfile(source, output / "assets" / name)
    shutil.copyfile(SITE / "style.css", output / "assets" / "style.css")
    sitemap = '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">' + "".join(
        f"<url><loc>{BASE_URL}{route(language)}</loc></url>" for language in LANGUAGES
    ) + "</urlset>\n"
    (output / "sitemap.xml").write_text(sitemap, encoding="utf-8", newline="\n")
    (output / ".nojekyll").write_bytes(b"")
    validate(output)
    return len(expected_files())


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Validate the existing build without changing it")
    args = parser.parse_args()
    if args.check:
        validate()
        print("Site validation passed.")
    else:
        print(f"Built and validated {build()} public files in {OUTPUT}.")
