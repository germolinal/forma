"""Tests for the parts of the entrypoint that do not need root or a network.

Run with: python3 -I forma/sandbox/test_entrypoint.py
"""

import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import entrypoint  # noqa: E402
from entrypoint import SandboxError  # noqa: E402


class ValidateRelPath(unittest.TestCase):
    def test_accepts_plain_relative_paths(self):
        for path in ["a.py", "data/ventas.csv", "out/charts/x.png", "ñandú.txt"]:
            self.assertEqual(entrypoint.validate_rel_path(path), path)

    def test_refuses_anything_that_could_leave_the_work_dir(self):
        for path in ["", "/etc/passwd", "../x", "a/../../x", "a/./b", "a//b", "a/", ".",
                     "a\\b", "a\x00b", "a\nb"]:
            with self.assertRaises(SandboxError, msg=repr(path)):
                entrypoint.validate_rel_path(path)

    def test_refuses_overlong_paths(self):
        with self.assertRaises(SandboxError):
            entrypoint.validate_rel_path("a" * (entrypoint.MAX_PATH_BYTES + 1))


class SniffMediaType(unittest.TestCase):
    def test_content_wins_over_the_name(self):
        png = b"\x89PNG\r\n\x1a\n" + b"\x00" * 10
        self.assertEqual(entrypoint.sniff_media_type("chart.txt", png), "image/png")
        self.assertEqual(entrypoint.sniff_media_type("x.png", b"hello"), "text/plain; charset=utf-8")

    def test_known_formats(self):
        self.assertEqual(entrypoint.sniff_media_type("a", b"%PDF-1.7"), "application/pdf")
        self.assertEqual(entrypoint.sniff_media_type("a", b"\xff\xd8\xff\xe0"), "image/jpeg")
        self.assertEqual(entrypoint.sniff_media_type("a", b"RIFF\x00\x00\x00\x00WEBPVP8"), "image/webp")

    def test_text_is_refined_by_extension_but_never_becomes_markup(self):
        self.assertEqual(entrypoint.sniff_media_type("r.csv", b"a,b\n1,2"), "text/csv; charset=utf-8")
        self.assertEqual(entrypoint.sniff_media_type("r.json", b"{}"), "application/json")
        self.assertEqual(entrypoint.sniff_media_type("p.html", b"<script>"), "text/plain; charset=utf-8")
        self.assertEqual(entrypoint.sniff_media_type("p.svg", b"<svg>"), "text/plain; charset=utf-8")

    def test_a_zip_called_xlsx_is_a_workbook(self):
        self.assertEqual(entrypoint.sniff_media_type("out/v.xlsx", b"PK\x03\x04rest"), entrypoint.XLSX_TYPE)
        self.assertEqual(entrypoint.sniff_media_type("out/v.zip", b"PK\x03\x04rest"), "application/zip")

    def test_a_zip_called_pptx_is_a_deck(self):
        self.assertEqual(entrypoint.sniff_media_type("out/v.PPTX", b"PK\x03\x04rest"), entrypoint.PPTX_TYPE)

    def test_binary_is_octet_stream(self):
        self.assertEqual(entrypoint.sniff_media_type("a.txt", b"\xff\xfe\x00"), "application/octet-stream")
        self.assertEqual(entrypoint.sniff_media_type("a.txt", b"abc\x00def"), "application/octet-stream")


class Multipart(unittest.TestCase):
    def test_file_goes_last_and_fields_are_kept(self):
        body, content_type = entrypoint.multipart_body(
            [("key", "runs/x/out/a.csv"), ("policy", "abc")], "a.csv", b"1,2", "text/csv"
        )
        boundary = content_type.split("boundary=")[1]
        self.assertTrue(body.endswith(f"\r\n--{boundary}--\r\n".encode()))
        self.assertLess(body.index(b'name="policy"'), body.index(b'name="file"'))
        self.assertIn(b"runs/x/out/a.csv", body)
        self.assertIn(b"1,2", body)


class CollectOutputs(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.out = os.path.join(self.tmp.name, "out")
        os.makedirs(self.out)
        self.uid = os.getuid()

    def tearDown(self):
        self.tmp.cleanup()

    def write(self, rel, data):
        path = os.path.join(self.out, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as f:
            f.write(data)
        return path

    def collect(self, **limits):
        args = {"max_files": 10, "max_file_bytes": 100, "max_total_bytes": 150}
        args.update(limits)
        return entrypoint.collect_outputs(self.out, self.uid, **args)

    def test_collects_nested_files(self):
        self.write("a.csv", b"1")
        self.write("charts/b.png", b"2")
        files, skipped = self.collect()
        self.assertEqual(sorted(p for p, _ in files), ["a.csv", "charts/b.png"])
        self.assertEqual(skipped, [])

    def test_symlinks_are_never_followed(self):
        secret = os.path.join(self.tmp.name, "secret")
        with open(secret, "wb") as f:
            f.write(b"manifest")
        os.symlink(secret, os.path.join(self.out, "leak.txt"))
        os.symlink(self.tmp.name, os.path.join(self.out, "dir"))
        files, skipped = self.collect()
        self.assertEqual(files, [])
        self.assertEqual([s["path"] for s in skipped], ["leak.txt"])

    def test_an_out_dir_that_is_a_symlink_is_refused(self):
        os.rmdir(self.out)
        os.symlink(self.tmp.name, self.out)
        files, skipped = self.collect()
        self.assertEqual(files, [])
        self.assertEqual(skipped[0]["reason"], "out is not a directory")

    def test_hard_links_are_refused(self):
        path = self.write("a.txt", b"1")
        os.link(path, os.path.join(self.out, "b.txt"))
        files, skipped = self.collect()
        self.assertEqual(files, [])
        self.assertEqual(len(skipped), 2)

    def test_files_owned_by_someone_else_are_refused(self):
        self.write("a.txt", b"1")
        files, skipped = entrypoint.collect_outputs(
            self.out, self.uid + 1, max_files=10, max_file_bytes=100, max_total_bytes=150
        )
        self.assertEqual(files, [])
        self.assertEqual(skipped[0]["reason"], "not a file the command created")

    def test_limits(self):
        self.write("big.bin", b"x" * 101)
        self.write("a.txt", b"x" * 100)
        self.write("b.txt", b"x" * 100)
        files, skipped = self.collect()
        self.assertEqual([p for p, _ in files], ["a.txt"])
        reasons = {s["path"]: s["reason"] for s in skipped}
        self.assertIn("larger than", reasons["big.bin"])
        self.assertIn("in total", reasons["b.txt"])

        files, skipped = self.collect(max_files=1, max_total_bytes=10_000)
        self.assertEqual(len(files), 1)
        self.assertTrue(any("output files" in s["reason"] for s in skipped))

    def test_missing_out_dir_is_no_outputs(self):
        os.rmdir(self.out)
        self.assertEqual(self.collect(), ([], []))


if __name__ == "__main__":
    unittest.main()
