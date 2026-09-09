"""Exercise the publishing boundary and common broken-site failure modes."""

from pathlib import Path
import struct
import stat
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch
import zlib

import build


def small_png():
    def chunk(kind, data):
        checksum = zlib.crc32(kind + data) & 0xFFFFFFFF
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", checksum)

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", 2, 2, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(b"\x00" + b"\x50\xa0\xf0" * 2 + b"\x00" + b"\x20\x40\x60" * 2))
        + chunk(b"IEND", b"")
    )


class PublicSiteTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        fixture = self.root / "sample.png"
        fixture.write_bytes(small_png())
        self.assets = {name: fixture for name in build.ASSETS}
        self.output = self.root / "public"
        build.build(self.output, self.assets)

    def edit_page(self, old, new, language="en"):
        path = self.output / build.route(language) / "index.html"
        text = path.read_text(encoding="utf-8")
        self.assertIn(old, text)
        path.write_text(text.replace(old, new, 1), encoding="utf-8")

    def test_build_has_only_public_files_and_all_locales(self):
        build.validate(self.output)
        for language in build.LANGUAGES:
            content = (self.output / build.route(language) / "index.html").read_text(encoding="utf-8")
            self.assertIn(f'<html lang="{language}">', content)
            self.assertIn("cargo run --release --locked", content)

    def test_internal_file_accidentally_added_to_artifact_is_rejected(self):
        (self.output / "private-notes.md").write_text("Should never be published.")
        with self.assertRaisesRegex(ValueError, "Deployment file mismatch"):
            build.validate(self.output)

    def test_missing_public_image_is_rejected(self):
        (self.output / "assets" / "reading-spread.png").unlink()
        with self.assertRaisesRegex(ValueError, "Deployment file mismatch"):
            build.validate(self.output)

    def test_wrong_canonical_is_rejected(self):
        self.edit_page('rel="canonical" href="' + build.BASE_URL + 'ko/"', 'rel="canonical" href="' + build.BASE_URL + '"', "ko")
        with self.assertRaisesRegex(ValueError, "canonical"):
            build.validate(self.output)

    def test_missing_language_alternate_is_rejected(self):
        self.edit_page('hreflang="ja"', 'hreflang="xx"')
        with self.assertRaisesRegex(ValueError, "alternates"):
            build.validate(self.output)

    def test_broken_anchor_is_rejected(self):
        self.edit_page('href="#quality"', 'href="#missing-section"')
        with self.assertRaisesRegex(ValueError, "Broken anchor"):
            build.validate(self.output)

    def test_nested_page_link_outside_project_is_rejected(self):
        self.edit_page('href="/SuiSuiView/ja/"', 'href="/ja/"', "ko")
        with self.assertRaisesRegex(ValueError, "escapes project"):
            build.validate(self.output)

    def test_missing_screenshot_description_is_rejected(self):
        self.edit_page('alt="SuiSuiView showing original geometric comic pages in two-page view"', 'alt=""')
        with self.assertRaisesRegex(ValueError, "image text"):
            build.validate(self.output)

    def test_script_injection_is_rejected(self):
        self.edit_page("</body>", '<script src="https://example.com/tracker.js"></script></body>')
        with self.assertRaisesRegex(ValueError, "active content"):
            build.validate(self.output)

    def test_wrong_image_dimensions_are_rejected(self):
        self.edit_page('width="2" height="2"', 'width="3" height="2"')
        with self.assertRaisesRegex(ValueError, "dimensions"):
            build.validate(self.output)

    def test_rebuild_is_deterministic(self):
        original = {name: (self.output / name).read_bytes() for name in build.expected_files()}
        build.build(self.output, self.assets)
        self.assertEqual(original, {name: (self.output / name).read_bytes() for name in build.expected_files()})

    def test_rebuild_preserves_unexpected_files_and_does_not_write(self):
        page = self.output / "index.html"
        page.write_text("Previous output", encoding="utf-8")
        unknown = self.output / "private-notes.md"
        unknown.write_text("User-owned data", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "unexpected files"):
            build.build(self.output, self.assets)
        self.assertEqual(unknown.read_text(encoding="utf-8"), "User-owned data")
        self.assertEqual(page.read_text(encoding="utf-8"), "Previous output")

    def test_windows_junction_attributes_are_rejected(self):
        info = SimpleNamespace(st_mode=stat.S_IFDIR, st_file_attributes=0x400)
        with patch.object(Path, "lstat", return_value=info):
            with self.assertRaisesRegex(ValueError, "reparse points"):
                build.build(self.output, self.assets)

    def test_redirected_child_directory_is_rejected_without_touching_target(self):
        outside = self.root / "outside"
        outside.mkdir()
        note = outside / "keep.txt"
        note.write_text("User-owned data", encoding="utf-8")
        nested_page = self.output / "ko" / "index.html"
        nested_page.unlink()
        nested_page.parent.rmdir()
        try:
            nested_page.parent.symlink_to(outside, target_is_directory=True)
        except OSError as error:
            self.skipTest(f"Creating symlinks is unavailable: {error}")
        with self.assertRaisesRegex(ValueError, "symlinks"):
            build.build(self.output, self.assets)
        self.assertEqual(note.read_text(encoding="utf-8"), "User-owned data")
        self.assertFalse((outside / "index.html").exists())

    def test_locale_copy_is_escaped_as_text(self):
        data = build.load_locales()["en"]
        data["intro"] = '<script>"hello"</script>'
        template = build.Template((build.SITE / "template.html").read_text(encoding="utf-8"))
        dimensions = {name: (2, 2) for name in build.ASSETS}
        rendered = build.render_page("en", data, template, dimensions)
        self.assertEqual(build.Page(rendered).attrs("script"), [])
        self.assertIn("&lt;script&gt;&quot;hello&quot;&lt;/script&gt;", rendered)


if __name__ == "__main__":
    unittest.main()
