#!/usr/bin/env python3
"""Installer verification with owned synthetic archives and an HTTPS curl double."""
import hashlib
import io
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest

INSTALLER=Path(__file__).with_name("install-journal.sh")
VERSION="0.1.0"
ASSET=f"ojo-journal-v{VERSION}-x86_64-unknown-linux-gnu.tar.gz"

class Installer(unittest.TestCase):
    def fixture(self,root,kind="valid"):
        package=root/"package"
        for name in ["LICENSE","NOTICE","Cargo.lock","manifest.json","licenses/dependency/NOTICE"]:
            path=package/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text("synthetic notice\n")
        for name in ["ojo-journal","oojuro","oojuro-native","oojurod"]:
            path=package/"bin"/name;path.parent.mkdir(parents=True,exist_ok=True)
            path.write_text(f"#!/bin/sh\nprintf '%s\\n' 'ojo-journal {VERSION}'\n");path.chmod(0o755)
        release=root/"release";release.mkdir()
        archive=release/ASSET
        with tarfile.open(archive,"w:gz") as tar:
            for path in sorted(package.rglob("*")):
                tar.add(path,arcname=str(path.relative_to(package)),recursive=False)
            if kind=="traversal":
                info=tarfile.TarInfo("../escape");info.size=4;tar.addfile(info,io.BytesIO(b"nope"))
            if kind=="symlink":
                info=tarfile.TarInfo("bin/escape");info.type=tarfile.SYMTYPE;info.linkname="/tmp/escape";tar.addfile(info)
        digest=hashlib.sha256(archive.read_bytes()).hexdigest()
        if kind=="checksum":digest="0"*64
        (release/"SHA256SUMS").write_text(f"{digest}  {ASSET}\n")
        tools=root/"tools";tools.mkdir()
        curl=tools/"curl"
        curl.write_text("#!"+sys.executable+"\n"+r'''
import os,pathlib,shutil,sys,urllib.parse
args=sys.argv[1:]
assert '--proto' in args and '=https' in args and '--tlsv1.2' in args
url=next(value for value in args if value.startswith('https://'))
assert '/ojo-journal-v0.1.0/' in url
destination=pathlib.Path(args[args.index('--output')+1])
shutil.copyfile(pathlib.Path(os.environ['JOURNAL_INSTALL_FIXTURE'])/pathlib.PurePosixPath(urllib.parse.urlsplit(url).path).name,destination)
''');curl.chmod(0o700)
        return {**os.environ,"PATH":str(tools)+os.pathsep+os.environ["PATH"],"JOURNAL_INSTALL_FIXTURE":str(release),"OJO_JOURNAL_RELEASE_BASE":"https://synthetic.invalid/releases/download","OJO_JOURNAL_PREFIX":str(root/"prefix with spaces"),"TMPDIR":str(root)}
    def test_verified_install_retains_all_binaries_and_licenses(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);env=self.fixture(root)
            output=subprocess.run(["sh",str(INSTALLER)],env=env,capture_output=True,text=True)
            self.assertEqual(output.returncode,0,output.stderr)
            prefix=Path(env["OJO_JOURNAL_PREFIX"])
            for name in ["ojo-journal","oojuro","oojuro-native","oojurod"]:self.assertTrue((prefix/"bin"/name).is_file())
            self.assertTrue((prefix/"share/ojo-journal/0.1.0/licenses/dependency/NOTICE").is_file())
            self.assertEqual(subprocess.check_output([str(prefix/"bin/ojo-journal"),"--version"],text=True).strip(),"ojo-journal 0.1.0")
            self.assertFalse(list(root.glob("ojo-journal-install.*")))
    def test_bad_checksums_links_and_traversal_fail_before_install(self):
        for kind in ["checksum","traversal","symlink"]:
            with self.subTest(kind=kind),tempfile.TemporaryDirectory() as directory:
                root=Path(directory);env=self.fixture(root,kind)
                output=subprocess.run(["sh",str(INSTALLER)],env=env,capture_output=True,text=True)
                self.assertNotEqual(output.returncode,0)
                self.assertFalse((Path(env["OJO_JOURNAL_PREFIX"])/"bin/ojo-journal").exists())
                self.assertFalse((root/"escape").exists())
                self.assertFalse(list(root.glob("ojo-journal-install.*")))
    def test_existing_symlink_destination_is_not_followed(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);env=self.fixture(root);prefix=Path(env["OJO_JOURNAL_PREFIX"])
            (prefix/"bin").mkdir(parents=True)
            original=root/"unrelated";original.write_text("preserve me")
            (prefix/"bin/ojo-journal").symlink_to(original)
            output=subprocess.run(["sh",str(INSTALLER)],env=env,capture_output=True,text=True)
            self.assertNotEqual(output.returncode,0)
            self.assertEqual(original.read_text(),"preserve me")
    def test_insecure_origin_is_refused_before_downloading(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);env=self.fixture(root);env["OJO_JOURNAL_RELEASE_BASE"]="http://synthetic.invalid"
            output=subprocess.run(["sh",str(INSTALLER)],env=env,capture_output=True,text=True)
            self.assertNotEqual(output.returncode,0)
            self.assertIn("HTTPS",output.stderr)

if __name__=="__main__":unittest.main()
