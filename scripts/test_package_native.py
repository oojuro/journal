import importlib.util
import json
from pathlib import Path
import tempfile
import tarfile
import unittest
import os
import subprocess
import sys
import io
from unittest import mock

spec = importlib.util.spec_from_file_location("package_native", Path(__file__).with_name("package_native.py"))
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class Distribution(unittest.TestCase):
    def test_git_engine_notices_and_dictionary_are_resolved_from_the_dependency(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, engine, rust, binaries = [root / name for name in ["source", "engine", "rust", "bin"]]
            files = {
                source / "LICENSE": "synthetic project license",
                source / "NOTICE": "synthetic project notice",
                source / "Cargo.lock": "version = 4\npackage = []\n",
                source / "licenses/MIT.txt": "synthetic supplement",
                engine / "LICENSE": "synthetic engine license",
                engine / "NOTICE": "synthetic engine notice",
                engine / "licenses/MIT.txt": "synthetic engine supplement",
                engine / "crates/oojuro-store/dict/README.md": "synthetic dictionary provenance",
                engine / "crates/oojuro-store/dict/starter-v1.zdict": "synthetic dictionary bytes",
                rust / "share/doc/rust/COPYRIGHT-library.html": "synthetic Rust attribution",
                rust / "share/doc/rust/licenses/MIT.txt": "synthetic Rust license",
                binaries / "oojuro": "synthetic binary with synthetic dictionary bytes",
                binaries / "oojurod": "synthetic recorder with synthetic dictionary bytes",
            }
            for path, text in files.items():
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(text)
            revision = "a" * 40
            dependency = {"name": "oojuro-store", "version": "0.1.0", "license": "Apache-2.0",
                          "source": "git+https://github.com/oojuro/oojuro.git?rev=" + revision + "#" + revision,
                          "manifest_path": str(engine / "crates/oojuro-store/Cargo.toml")}

            def run(*args):
                if args[:2] == ("cargo", "metadata"):
                    return json.dumps({"packages": [dependency]})
                if args[:2] == ("cargo", "tree"):
                    return "oojuro-store v0.1.0"
                if args[:2] == ("rustc", "--print"):
                    return str(rust)
                if args[:2] == ("git", "status"):
                    return ""
                return "synthetic toolchain or source revision"

            out = root / "package"
            with mock.patch.object(package, "ROOT", source), mock.patch.object(package, "run", run):
                package.package(binaries, out, "x86_64-unknown-linux-gnu")
                self.assertEqual((out / "licenses/dictionary-provenance.md").read_text(),
                                 "synthetic dictionary provenance")
                self.assertTrue((out / "licenses" / ("oojuro-engine-" + revision) / "NOTICE").is_file())
                manifest = json.loads((out / "manifest.json").read_text())
                self.assertTrue(all(value["dictionaryIncluded"] for value in manifest["binaries"].values()))
                dependency["source"] = "git+https://example.invalid/unreviewed#" + revision
                with self.assertRaisesRegex(ValueError, "unreviewed git dependency"):
                    package.package(binaries, root / "unreviewed", "x86_64-unknown-linux-gnu")

    def test_empty_archive_notices_are_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "example-1.crate"
            with tarfile.open(archive, "w:gz") as target:
                member = tarfile.TarInfo("example-1/LICENSE")
                member.size = 3
                target.addfile(member, io.BytesIO(b" \n\t"))
            with self.assertRaises(ValueError):
                package.crate_notices(archive, package.sha(archive))

    def test_release_build_prepares_cache_before_building_or_packaging(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tools, log = root / "tools", root / "calls.jsonl"
            tools.mkdir()
            helper = "#!" + sys.executable + "\n" + r"""
import json, os, sys
from pathlib import Path
name = Path(sys.argv[0]).name
if name == "git":
    sys.exit(0)
with open(os.environ['NATIVE_TEST_LOG'], 'a') as log:
    log.write(json.dumps([name, *sys.argv[1:]]) + '\n')
if name == 'cargo' and sys.argv[1] == 'fetch' and os.environ.get('NATIVE_TEST_FETCH_FAIL'):
    sys.exit(23)
"""
            for name in ["git", "cargo", "python3"]:
                file = tools / name
                file.write_text(helper)
                file.chmod(0o700)
            target = root / "target with spaces"
            env = {**os.environ, "PATH": str(tools) + os.pathsep + os.environ['PATH'],
                   "NATIVE_TEST_LOG": str(log), "CARGO_TARGET_DIR": str(target)}
            script = Path(__file__).with_name("build_native.sh")
            result = subprocess.run(["bash", str(script), str(root / "fresh output")], env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            calls = [json.loads(line) for line in log.read_text().splitlines()]
            self.assertEqual([call[:2] for call in calls], [["cargo", "fetch"], ["cargo", "build"], ["python3", "scripts/package_native.py"]])
            self.assertIn("--locked", calls[0])
            self.assertEqual(calls[2][2], str(target / "x86_64-unknown-linux-gnu/release"))
            log.unlink()
            result = subprocess.run(["bash", str(script), str(root / "fresh output")], env={**env, "NATIVE_TEST_FETCH_FAIL": "1"}, capture_output=True, text=True)
            self.assertEqual(result.returncode, 23)
            self.assertEqual(len(log.read_text().splitlines()), 1)

    def test_registry_archive_is_verified_against_lock_checksum(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            notice = root / "LICENSE"
            notice.write_text("synthetic upstream copyright")
            archive = root / "example-1.crate"
            with tarfile.open(archive, "w:gz") as target:
                target.add(notice, arcname="example-1/LICENSE")
            checksum = package.sha(archive)
            self.assertEqual(package.crate_notices(archive, checksum), {"LICENSE": notice.read_bytes()})
            archive.write_bytes(archive.read_bytes() + b"corrupt")
            with self.assertRaises(ValueError):
                package.crate_notices(archive, checksum)

    def test_notice_verifier_rejects_missing_changed_and_escaping_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            files = {}
            for name in ["LICENSE", "NOTICE", "Cargo.lock", "bin/oojuro", "bin/oojurod", "licenses/nested/NOTICE"]:
                dest = out / name
                dest.parent.mkdir(parents=True, exist_ok=True)
                dest.write_text("synthetic " + name)
                files[name] = package.sha(dest)
            (out / "manifest.json").write_text(json.dumps({"files": files}))
            package.verify(out)
            (out / "NOTICE").write_text("deleted attribution")
            with self.assertRaises(ValueError):
                package.verify(out)
            (out / "NOTICE").unlink()
            with self.assertRaises(FileNotFoundError):
                package.verify(out)
            files = {"../escape": "fake"}
            (out / "manifest.json").write_text(json.dumps({"files": files}))
            with self.assertRaises(ValueError):
                package.verify(out)

    def test_nested_legal_files_are_included(self):
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            for name in ["LICENSE", "vendor/NOTICE", "vendor/COPYING", "vendor/code.c"]:
                dest = out / name
                dest.parent.mkdir(parents=True, exist_ok=True)
                dest.write_text("synthetic")
            self.assertEqual({str(p.relative_to(out)) for p in package.legal_files(out)}, {"LICENSE", "vendor/NOTICE", "vendor/COPYING"})


if __name__ == "__main__":
    unittest.main()
