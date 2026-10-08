import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location("controlled_codex_build", Path(__file__).with_name("build.py"))
build = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build)


class PinnedSourceTests(unittest.TestCase):
    def test_changed_archive_is_rejected_before_extraction(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / "source.tar.gz"
            archive.write_bytes(b"untrusted archive")
            destination = root / "sources"
            with self.assertRaisesRegex(ValueError, "digest"):
                build.prepare(archive, destination)
            self.assertFalse(destination.exists())


if __name__ == "__main__":
    unittest.main()
