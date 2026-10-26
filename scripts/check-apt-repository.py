#!/usr/bin/env python3
"""Test local APT preparation with real GPG/APT, no external network/root/install.

All keys, DEBs, APT configuration, package lists, cache and status live inside a
single test-owned temporary tree. The fixture is not the Giveaway Scout app.
Requires Linux, an unprivileged user, cc, dpkg-deb, binutils, apt-utils and gnupg.
"""

import argparse
from datetime import datetime, timedelta, timezone
from email.utils import format_datetime
import gzip
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import json
import os
from pathlib import Path
import pwd
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest


SCRIPT = Path(__file__).with_name("prepare-apt-repository.py")
spec = importlib.util.spec_from_file_location("apt_preparation", SCRIPT)
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)


class AptRepositoryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if sys.platform != "linux" or os.geteuid() == 0:
            raise RuntimeError("Run these checks on Linux without sudo/root")
        cls.temporary = tempfile.TemporaryDirectory(prefix="giveaway-scout-apt-check-")
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.root = Path(cls.temporary.name)
        cls.home = cls.root / "gnupg"
        cls.home.mkdir(mode=0o700)
        cls.addClassCleanup(lambda: subprocess.run(
            ["gpgconf", "--homedir", str(cls.home), "--kill", "gpg-agent"],
            capture_output=True, timeout=10,
        ))
        cls.gpg = ["gpg", "--no-options", "--batch", "--homedir", str(cls.home)]
        # Keep key creation away from the Release timestamp's second boundary.
        # The real builder still rejects any Release older than its signing key.
        key_epoch = int(time.time()) - 60
        for label in ("signer", "untrusted"):
            builder.run([*cls.gpg, "--pinentry-mode", "loopback", "--passphrase", "",
                         "--faked-system-time", str(key_epoch),
                         "--quick-generate-key", f"Ephemeral {label} <{label}@example.invalid>",
                         "ed25519", "sign", "1d"])
        records = builder.run([*cls.gpg, "--with-colons", "--list-secret-keys"]).decode()
        cls.key, cls.wrong_key = [line.split(":")[9] for line in records.splitlines()
                                  if line.startswith("fpr:")]
        cls.epoch = int(time.time())
        cls.arch = builder.run(["dpkg", "--print-architecture"]).decode().strip()
        if cls.arch not in builder.MACHINES:
            raise RuntimeError("These checks require an amd64 or arm64 Linux host")
        source = cls.root / "fixture.c"
        source.write_text("int main(void) { return 0; }\n")
        cls.elf = cls.root / "fixture.elf"
        builder.run(["cc", "-Wl,--build-id=none", str(source), "-o", str(cls.elf)])
        cls.fixture = cls.make_deb(cls.root / "base", cls.root / "fixture.deb")
        cls.wrong_public = cls.root / "untrusted.gpg"
        cls.wrong_public.write_bytes(builder.run([*cls.gpg, "--export", cls.wrong_key]))

    @classmethod
    def make_deb(cls, directory, output, *, version="0.1.0", package="giveaway-scout",
                 arch=None, binary=None, include_app=True):
        control = directory / "DEBIAN"
        control.mkdir(parents=True)
        (control / "control").write_text(
            f"Package: {package}\nVersion: {version}\nArchitecture: {arch or cls.arch}\n"
            "Maintainer: Ephemeral fixture <fixture@example.invalid>\n"
            "Section: utils\nPriority: optional\nDescription: Never an application release\n"
        )
        if include_app:
            app = directory / "usr/bin/giveaway-scout"
            app.parent.mkdir(parents=True)
            shutil.copyfile(binary or cls.elf, app)
            app.chmod(0o755)
        builder.run(["dpkg-deb", "--root-owner-group", "--build", str(directory), str(output)])
        return output

    def setUp(self):
        self.case = Path(tempfile.mkdtemp(prefix="case-", dir=self.root))

    def arguments(self, output, **overrides):
        values = dict(deb=[self.fixture], suite="bookworm", output=output,
                      gpg_home=self.home, signing_key=self.key, epoch=self.epoch, valid_days=14)
        values.update(overrides)
        return argparse.Namespace(**values)

    def prepare(self, name="repository", **overrides):
        return builder.prepare(self.arguments(self.case / name, **overrides))

    def apt_environment(self, repository, *, key=None, name="client", suite="bookworm", uri=None):
        root = self.case / name
        root.mkdir()
        for directory in ("lists/partial", "cache/archives/partial", "log", "trusted-empty"):
            (root / directory).mkdir(parents=True)
        (root / "status").write_text("")
        key = key or repository / "giveaway-scout-archive-keyring.gpg"
        (root / "sources.list").write_text(
            f"deb [arch={self.arch} signed-by={key}] {uri or repository.as_uri()} {suite} main\n"
        )
        # APT_CONFIG is read before /etc/apt/apt.conf{,.d}; disable those paths
        # here, not only with late CLI flags, to avoid loading system hooks.
        options = {
            "Dir::Etc::parts": "-", "Dir::Etc::main": "-",
            "Dir::Etc::sourcelist": str(root / "sources.list"),
            "Dir::Etc::sourceparts": "-", "Dir::Etc::trusted": str(root / "missing.gpg"),
            "Dir::Etc::trustedparts": str(root / "trusted-empty"),
            "Dir::State::lists": str(root / "lists"), "Dir::State::status": str(root / "status"),
            "Dir::State::extended_states": str(root / "extended_states"),
            "Dir::Cache": str(root / "cache"), "Dir::Cache::pkgcache": "",
            "Dir::Cache::srcpkgcache": "", "Dir::Log": str(root / "log"),
            "APT::Sandbox::User": pwd.getpwuid(os.getuid()).pw_name,
            "APT::Architecture": self.arch, "Acquire::Languages": "none",
            "Acquire::Retries": "0", "Acquire::AllowInsecureRepositories": "false",
            "Acquire::AllowDowngradeToInsecureRepositories": "false",
            "Acquire::Check-Date": "true", "Acquire::Check-Valid-Until": "true",
            "APT::Get::AllowUnauthenticated": "false", "APT::Update::Error-Mode": "any",
            "Debug::NoLocking": "true",
        }
        config = root / "apt.conf"
        config.write_text("\n".join(f'{key} "{value}";' for key, value in options.items()) + "\n")
        return root, {**os.environ, "APT_CONFIG": str(config), "LC_ALL": "C"}

    def apt(self, client, *arguments, executable="apt-get", success=True):
        root, env = client
        result = subprocess.run([executable, *arguments], cwd=root, env=env,
                                capture_output=True, text=True, timeout=30)
        details = result.stdout + result.stderr
        if success:
            self.assertEqual(result.returncode, 0, details)
        else:
            self.assertNotEqual(result.returncode, 0, details)
        return details

    def test_real_apt_accepts_signed_indexes_and_downloads_the_exact_package(self):
        repository = self.prepare()
        client = self.apt_environment(repository)
        self.apt(client, "update")
        policy = self.apt(client, "policy", "giveaway-scout", executable="apt-cache")
        self.assertIn("0.1.0", policy)
        self.assertIn("Installed: (none)", policy)
        configuration = self.apt(client, "dump", executable="apt-config")
        self.assertNotIn("::Post-Invoke", configuration)
        self.assertNotIn("::Pre-Invoke", configuration)
        self.apt(client, "download", "giveaway-scout=0.1.0")
        downloaded, = client[0].glob("*.deb")
        self.assertEqual(builder.digest(downloaded), builder.digest(self.fixture))
        manifest = json.loads((repository / "repository.json").read_text())
        self.assertEqual(manifest["signingFingerprint"], self.key)
        self.assertEqual(manifest["architectures"], [self.arch])
        self.assertEqual(manifest["nativeAcceptance"], "not-proven-by-repository-preparation")
        self.assertEqual((client[0] / "status").read_bytes(), b"")
        self.assertFalse(any("private" in path.name for path in repository.rglob("*")))
        binary = repository / f"dists/bookworm/main/binary-{self.arch}"
        self.assertEqual(gzip.decompress((binary / "Packages.gz").read_bytes()),
                         (binary / "Packages").read_bytes())
        for path in (binary / "by-hash").glob("*/*"):
            algorithm = path.parent.name.lower()
            self.assertEqual(builder.digest(path, algorithm), path.name)

    def test_real_apt_uses_content_addressed_indexes_over_loopback_http(self):
        repository = self.prepare()
        requests = []

        class Handler(SimpleHTTPRequestHandler):
            def __init__(self, *args, **kwargs):
                super().__init__(*args, directory=str(repository), **kwargs)

            def do_GET(self):
                requests.append(self.path)
                super().do_GET()

            def log_message(self, *args):
                pass

        with ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
            thread = threading.Thread(target=server.serve_forever,
                                      kwargs={"poll_interval": 0.05}, daemon=True)
            thread.start()
            try:
                uri = f"http://127.0.0.1:{server.server_port}"
                client = self.apt_environment(repository, uri=uri)
                self.apt(client, "update")
                self.apt(client, "download", "giveaway-scout=0.1.0")
                self.assertTrue(any("/by-hash/" in path for path in requests), requests)
                downloaded, = client[0].glob("*.deb")
                self.assertEqual(builder.digest(downloaded), builder.digest(self.fixture))
            finally:
                server.shutdown()
                thread.join(timeout=5)
                self.assertFalse(thread.is_alive())

    def test_real_apt_accepts_the_separate_noble_suite(self):
        repository = self.prepare(suite="noble")
        client = self.apt_environment(repository, suite="noble")
        self.apt(client, "update")
        self.apt(client, "download", "giveaway-scout=0.1.0")
        self.assertTrue((repository / "dists/noble/InRelease").is_file())
        self.assertFalse((repository / "dists/bookworm").exists())

    def test_real_apt_rejects_a_wrong_signing_key(self):
        repository = self.prepare()
        client = self.apt_environment(repository, key=self.wrong_public)
        error = self.apt(client, "update", success=False)
        self.assertRegex(error, r"NO_PUBKEY|No public key|Missing key")

    def test_real_apt_rejects_modified_signed_metadata(self):
        repository = self.prepare()
        release = repository / "dists/bookworm/InRelease"
        original = release.read_bytes()
        changed = original.replace(b"Giveaway Scout signed packages", b"modified unsigned description")
        self.assertNotEqual(changed, original)
        release.write_bytes(changed)
        error = self.apt(self.apt_environment(repository), "update", success=False)
        self.assertRegex(error, r"BADSIG|Bad signature|bad signature")

    def test_real_apt_rejects_modified_package_bytes(self):
        repository = self.prepare()
        client = self.apt_environment(repository)
        self.apt(client, "update")
        package, = repository.glob("pool/**/*.deb")
        package.write_bytes(package.read_bytes() + b"tampered")
        error = self.apt(client, "download", "giveaway-scout=0.1.0", success=False)
        self.assertRegex(error, r"Hash Sum mismatch|File has unexpected size|hash.*mismatch")

    def test_real_apt_rejects_modified_indexes_even_with_an_unchanged_release(self):
        repository = self.prepare()
        binary = repository / f"dists/bookworm/main/binary-{self.arch}"
        altered = b"Package: not-the-signed-package\nVersion: 9\nArchitecture: " + self.arch.encode() + b"\n\n"
        for path in binary.rglob("*"):
            if path.is_file():
                # Mutate both conventional and by-hash variants; otherwise APT
                # correctly uses the still-authentic content-addressed copy.
                path.write_bytes(gzip.compress(altered, mtime=0) if path.read_bytes().startswith(b"\x1f\x8b") else altered)
        error = self.apt(self.apt_environment(repository), "update", success=False)
        self.assertRegex(error, r"Hash Sum mismatch|File has unexpected size|hash.*mismatch")

    def test_real_apt_rejects_an_expired_but_correctly_signed_release(self):
        repository = self.prepare()
        distribution = repository / "dists/bookworm"
        release = distribution / "Release"
        expired = format_datetime(datetime.now(timezone.utc) - timedelta(days=1), usegmt=True)
        release.write_text(re.sub(r"^Valid-Until:.*$", "Valid-Until: " + expired,
                                 release.read_text(), flags=re.MULTILINE))
        for name, action in (("InRelease", "--clearsign"), ("Release.gpg", "--detach-sign")):
            builder.run([*self.gpg, "--yes", "--local-user", self.key + "!", "--digest-algo", "SHA256",
                         "--output", str(distribution / name), action, str(release)])
        error = self.apt(self.apt_environment(repository), "update", success=False)
        self.assertRegex(error, r"expired|Expired")

    def test_fixed_epoch_makes_ed25519_fixture_snapshots_identical(self):
        first = self.prepare("first")
        second = self.prepare("second")
        def files(root):
            return {str(path.relative_to(root)): builder.digest(path)
                    for path in root.rglob("*") if path.is_file()}
        self.assertEqual(files(first), files(second))

    def test_existing_output_is_preserved_and_short_key_ids_are_rejected(self):
        output = self.case / "existing"
        output.mkdir()
        marker = output / "keep.txt"
        marker.write_text("existing user data")
        with self.assertRaisesRegex(ValueError, "already exists"):
            builder.prepare(self.arguments(output))
        self.assertEqual(marker.read_text(), "existing user data")
        with self.assertRaisesRegex(ValueError, "full fingerprint"):
            self.prepare(signing_key=self.key[-16:])

    def test_expired_signing_key_cannot_be_resurrected_by_backdating(self):
        epoch = int(time.time()) - 2 * 86400
        builder.run([*self.gpg, "--pinentry-mode", "loopback", "--passphrase", "",
                     "--faked-system-time", str(epoch), "--quick-generate-key",
                     "Ephemeral expired <expired@example.invalid>", "ed25519", "sign", "1d"])
        records = builder.run([*self.gpg, "--with-colons", "--list-secret-keys", "expired@example.invalid"]).decode()
        expired, = [line.split(":")[9] for line in records.splitlines() if line.startswith("fpr:")]
        with self.assertRaisesRegex(ValueError, "Signing key is expired"):
            self.prepare(signing_key=expired, epoch=epoch)
        self.assertFalse((self.case / "repository").exists())

    def test_release_timestamps_cannot_be_expired_or_from_the_future(self):
        for options in ({"epoch": int(time.time()) + 3600},
                        {"epoch": int(time.time()) - 31 * 86400}, {"valid_days": 0}, {"valid_days": 31}):
            with self.subTest(options=options):
                with self.assertRaisesRegex(ValueError, "timestamp|validity"):
                    self.prepare(**options)
                self.assertFalse((self.case / "repository").exists())

    def test_release_timestamp_cannot_precede_its_signing_key(self):
        records = builder.run([*self.gpg, "--with-colons", "--list-secret-keys", self.key]).decode()
        created, = [int(line.split(":")[5]) for line in records.splitlines() if line.startswith("sec:")]
        with self.assertRaisesRegex(ValueError, "creation time.*newer than Release timestamp"):
            self.prepare(epoch=created - 1)
        self.assertFalse((self.case / "repository").exists())

    def test_index_requires_the_exact_copied_file_hash_and_no_duplicate_fields(self):
        package = {"version": "0.1.0", "architecture": self.arch, "file": "pool/fixture.deb",
                   "size": 3, "sha256": "abc"}
        data = (f"Package: giveaway-scout\nVersion: 0.1.0\nArchitecture: {self.arch}\n"
                "Filename: pool/fixture.deb\nSize: 3\nSHA256: abc\n\n").encode()
        builder.validate_index(data, package)
        for changed in (data + b"SHA256: abc\n", data.replace(b"abc", b"changed"),
                        data.replace(b"pool/fixture.deb", b"https://untrusted.invalid/package.deb")):
            with self.subTest(changed=changed):
                with self.assertRaisesRegex(ValueError, "Generated package index"):
                    builder.validate_index(changed, package)

    def test_rejects_duplicate_architectures_and_inconsistent_versions(self):
        with self.assertRaisesRegex(ValueError, "one package per architecture"):
            self.prepare(deb=[self.fixture, self.fixture])
        other = self.make_deb(self.case / "other", self.case / "other.deb", version="0.2.0",
                              arch="arm64" if self.arch == "amd64" else "amd64")
        with self.assertRaisesRegex(ValueError, "same version"):
            self.prepare(deb=[self.fixture, other])

    def test_rejects_other_packages_wrong_elf_architecture_and_missing_application(self):
        for label, options, error in (
            ("other-package", {"package": "other-application"}, "Only giveaway-scout"),
            ("wrong-arch", {"arch": "arm64" if self.arch == "amd64" else "amd64"}, "architecture"),
            ("missing-app", {"include_app": False}, "does not contain"),
        ):
            with self.subTest(label=label):
                package = self.make_deb(self.case / label, self.case / (label + ".deb"), **options)
                with self.assertRaisesRegex(ValueError, error):
                    self.prepare(deb=[package])
                self.assertFalse((self.case / "repository").exists())

    def test_rejects_a_glibc_requirement_newer_than_the_declared_suite(self):
        content = self.elf.read_bytes()
        self.assertIn(b"GLIBC_2.34", content)
        changed = self.case / "newer-glibc.elf"
        changed.write_bytes(content.replace(b"GLIBC_2.34", b"GLIBC_9.99"))
        package = self.make_deb(self.case / "newer-glibc", self.case / "newer.deb", binary=changed)
        for suite in ("bookworm", "noble"):
            with self.subTest(suite=suite):
                with self.assertRaisesRegex(ValueError, "GLIBC 9.99 exceeds suite limit"):
                    self.prepare(deb=[package], suite=suite)
                self.assertFalse((self.case / "repository").exists())

    def test_command_line_builds_only_a_new_local_snapshot(self):
        output = self.case / "cli snapshot"
        result = subprocess.run([
            sys.executable, "-B", str(SCRIPT), "--deb", str(self.fixture), "--suite", "bookworm",
            "--output", str(output), "--gpg-home", str(self.home), "--signing-key", self.key,
            "--epoch", str(self.epoch),
        ], capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Nothing was published, installed", result.stdout)
        self.assertTrue((output / "dists/bookworm/InRelease").is_file())


if __name__ == "__main__":
    unittest.main(verbosity=2)
