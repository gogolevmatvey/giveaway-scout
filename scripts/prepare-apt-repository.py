#!/usr/bin/env python3
"""Prepare a local, signed Giveaway Scout APT snapshot; never publish/install it.

Requires Linux, Python 3.11+, dpkg-deb, readelf, apt-ftparchive, gpg and gpgv.
The caller supplies an existing signing key in an explicit GnuPG home. This
script never generates, imports, exports or prints a private key.
"""

import argparse
from contextlib import suppress
from datetime import datetime, timezone
from email.utils import format_datetime
import gzip
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import time


PACKAGE = "giveaway-scout"
MACHINES = {"amd64": 62, "arm64": 183}
SUITES = {"bookworm": (2, 36), "noble": (2, 39)}
MAX_ELF_BYTES = 256 * 1024 * 1024
MAX_UNPACKED_BYTES = 1024 * 1024 * 1024


def run(command, *, cwd=None, env=None):
    result = subprocess.run(command, cwd=cwd, env=env, capture_output=True, timeout=120)
    if result.returncode:
        raise ValueError(f"{command[0]} failed: {result.stderr.decode(errors='replace').strip()}")
    return result.stdout


def inspect_package(path):
    fields = [run(["dpkg-deb", "--field", str(path), field]).decode().strip()
              for field in ("Package", "Version", "Architecture")]
    package, version, arch = fields
    if package != PACKAGE or arch not in MACHINES:
        raise ValueError("Only giveaway-scout DEBs for amd64/arm64 are accepted")
    if not re.fullmatch(r"[0-9][0-9A-Za-z.+:~\-]*", version):
        raise ValueError("Invalid Debian package version")
    return version, arch


def inspect_abi(package, arch, maximum):
    """Read regular ELF members without extracting archive-controlled paths."""
    found_app = False
    required = (0, 0)
    unpacked = 0
    process = subprocess.Popen(
        ["dpkg-deb", "--fsys-tarfile", str(package)], stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    try:
        with tempfile.TemporaryDirectory(prefix="giveaway-scout-elf-") as temporary:
            elf = Path(temporary) / "member.elf"
            with tarfile.open(fileobj=process.stdout, mode="r|") as archive:
                for index, member in enumerate(archive):
                    name = PurePosixPath(member.name)
                    if name.is_absolute() or ".." in name.parts or index > 20_000:
                        raise ValueError("Unsafe or excessively large DEB archive")
                    if not member.isfile():
                        continue
                    unpacked += member.size
                    if member.size > MAX_ELF_BYTES or unpacked > MAX_UNPACKED_BYTES:
                        raise ValueError("DEB exceeds the unpacked size limit")
                    content = archive.extractfile(member)
                    header = content.read(64)
                    is_app = str(name) == "usr/bin/giveaway-scout"
                    if not header.startswith(b"\x7fELF"):
                        if is_app:
                            raise ValueError("The application must be a regular ELF executable")
                        continue
                    if (len(header) < 64 or header[4:7] != b"\x02\x01\x01"
                            or struct.unpack_from("<H", header, 16)[0] not in (2, 3)
                            or struct.unpack_from("<H", header, 18)[0] != MACHINES[arch]):
                        raise ValueError("ELF architecture does not match the DEB architecture")
                    found_app |= is_app
                    with elf.open("wb") as destination:
                        destination.write(header)
                        shutil.copyfileobj(content, destination)
                    versions = run(["readelf", "--wide", "--version-info", str(elf)]).decode()
                    if "GLIBC_PRIVATE" in versions:
                        raise ValueError("Private GLIBC ABI is not supported")
                    for major, minor in re.findall(r"\bGLIBC_(\d+)\.(\d+)(?:\b|\.)", versions):
                        required = max(required, (int(major), int(minor)))
            process.stdout.close()
            if process.wait(timeout=10):
                raise ValueError("Could not read the DEB filesystem archive")
    finally:
        process.stdout.close()
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
    if not found_app:
        raise ValueError("DEB does not contain usr/bin/giveaway-scout as a regular ELF file")
    if required > maximum:
        raise ValueError(f"GLIBC {required[0]}.{required[1]} exceeds suite limit "
                         f"{maximum[0]}.{maximum[1]}; rebuild on the older baseline")
    return ".".join(map(str, required)) if required != (0, 0) else None


def digest(path, algorithm="sha256"):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, algorithm).hexdigest()


def validate_index(data, package):
    expected = {"Package": PACKAGE, "Version": package["version"],
                "Architecture": package["architecture"], "Filename": package["file"],
                "Size": str(package["size"]), "SHA256": package["sha256"]}
    text = data.decode("utf-8")
    for field, value in expected.items():
        if re.findall(r"^" + field + r": ([^\n]*)$", text, re.MULTILINE) != [value]:
            raise ValueError(f"Generated package index has missing, duplicate or incorrect {field}")


def release_metadata(root, indexes, suite, architectures, epoch, valid_days):
    def date(value):
        return format_datetime(datetime.fromtimestamp(value, timezone.utc), usegmt=True)
    headers = [
        "Origin: Giveaway Scout", "Label: Giveaway Scout", f"Suite: {suite}",
        f"Codename: {suite}", f"Date: {date(epoch)}",
        f"Valid-Until: {date(epoch + valid_days * 86400)}",
        "Architectures: " + " ".join(architectures), "Components: main",
        "Description: Giveaway Scout signed packages", "Acquire-By-Hash: yes",
    ]
    for algorithm, label in (("sha256", "SHA256"), ("sha512", "SHA512")):
        headers.append(label + ":")
        for path in sorted(indexes):
            headers.append(f" {digest(path, algorithm)} {path.stat().st_size:16d} "
                           + path.relative_to(root).as_posix())
    return ("\n".join(headers) + "\n").encode()


def prepare(args):
    if not re.fullmatch(r"(?:[0-9A-Fa-f]{40}|[0-9A-Fa-f]{64})", args.signing_key):
        raise ValueError("Use the full fingerprint of the exact signing key, not a short key ID")
    fingerprint = args.signing_key.upper()
    home = args.gpg_home.resolve(strict=True)
    if not home.is_dir():
        raise ValueError("An explicit existing GnuPG home is required")
    output = args.output.absolute()
    if output.exists() or output.is_symlink():
        raise ValueError("Output already exists; refusing to replace any existing repository/data")
    if not output.parent.is_dir():
        raise ValueError("Create the output parent directory explicitly first")
    if not 1 <= args.valid_days <= 30:
        raise ValueError("Release validity must be 1-30 days")
    epoch = args.epoch
    if epoch is None:
        epoch = int(os.environ.get("SOURCE_DATE_EPOCH", str(int(time.time()))))
    if epoch < 0 or epoch > time.time() + 5 or epoch + args.valid_days * 86400 <= time.time():
        raise ValueError("Release timestamp is invalid, in the future or already expired")
    inputs = []
    versions = set()
    architectures = set()
    for source in args.deb:
        source = source.resolve(strict=True)
        if not source.is_file():
            raise ValueError("DEB input must be a regular file")
        version, arch = inspect_package(source)
        if arch in architectures:
            raise ValueError("Provide exactly one package per architecture")
        versions.add(version)
        architectures.add(arch)
        inputs.append((source, version, arch))
    if not inputs or len(versions) != 1:
        raise ValueError("All architecture packages must have the same version")
    gpg = ["gpg", "--no-options", "--batch", "--homedir", str(home)]
    keys = run([*gpg, "--with-colons", "--list-secret-keys", fingerprint]).decode()
    selected = None
    record = None
    primary_invalid = False
    for line in keys.splitlines():
        fields = line.split(":")
        if fields[0] in ("sec", "ssb"):
            record = fields
            if fields[0] == "sec":
                primary_invalid = fields[1] in ("r", "e", "d", "i")
        elif fields[0] == "fpr" and fields[9] == fingerprint:
            selected = record
    if selected is None:
        raise ValueError("The exact signing key is not available in the selected GnuPG home")
    # Backdating reproducible signatures must not resurrect an expired/revoked
    # publisher key. Validate current validity independently of --faked-system-time.
    if (primary_invalid or selected[1] in ("r", "e", "d", "i")
            or (selected[6] and int(selected[6]) <= time.time())):
        raise ValueError("Signing key is expired, revoked or disabled")
    if int(selected[5]) > epoch:
        raise ValueError(f"Signing key creation time {selected[5]} is newer than Release timestamp {epoch}")
    architectures = sorted(architectures)
    with tempfile.TemporaryDirectory(prefix=".giveaway-scout-apt-", dir=output.parent) as temporary:
        stage = Path(temporary)
        packages = []
        for source, version, arch in sorted(inputs, key=lambda item: item[2]):
            pool = stage / "pool" / "main" / "g" / PACKAGE / arch
            pool.mkdir(parents=True)
            target = pool / f"{PACKAGE}_{version}_{arch}.deb"
            shutil.copyfile(source, target)
            if inspect_package(target) != (version, arch):
                raise ValueError("Package metadata changed while copying")
            glibc = inspect_abi(target, arch, SUITES[args.suite])
            packages.append({"version": version, "architecture": arch,
                             "file": target.relative_to(stage).as_posix(),
                             "size": target.stat().st_size, "sha256": digest(target),
                             "requiredGlibc": glibc})
        distribution = stage / "dists" / args.suite
        indexes = []
        apt_config = stage / ".apt-preparation.conf"
        apt_config.write_text('Dir::Etc::parts "-";\nDir::Etc::main "-";\n')
        apt_env = {**os.environ, "APT_CONFIG": str(apt_config)}
        for arch in architectures:
            binary = distribution / "main" / f"binary-{arch}"
            binary.mkdir(parents=True)
            pool = Path("pool/main/g") / PACKAGE / arch
            index = binary / "Packages"
            data = run(["apt-ftparchive", "packages", pool.as_posix()], cwd=stage, env=apt_env)
            package = next(package for package in packages if package["architecture"] == arch)
            validate_index(data, package)
            index.write_bytes(data)
            compressed = binary / "Packages.gz"
            compressed.write_bytes(gzip.compress(index.read_bytes(), mtime=0))
            for path in (index, compressed):
                indexes.append(path)
                for algorithm, label in (("sha256", "SHA256"), ("sha512", "SHA512")):
                    by_hash = binary / "by-hash" / label / digest(path, algorithm)
                    by_hash.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(path, by_hash)
        apt_config.unlink()
        release = distribution / "Release"
        release.write_bytes(release_metadata(distribution, indexes, args.suite, architectures,
                                             epoch, args.valid_days))
        public_key = stage / "giveaway-scout-archive-keyring.gpg"
        public_key.write_bytes(run([*gpg, "--export-options", "export-minimal", "--export", fingerprint]))
        if not public_key.stat().st_size:
            raise ValueError("Public key export was empty")
        signing = [*gpg, "--local-user", fingerprint + "!", "--digest-algo", "SHA256",
                   "--faked-system-time", str(epoch)]
        run([*signing, "--output", str(distribution / "InRelease"), "--clearsign", str(release)])
        run([*signing, "--output", str(distribution / "Release.gpg"), "--detach-sign", str(release)])
        # No ambient trust store/configuration or automatic key retrieval.
        with tempfile.TemporaryDirectory(prefix="giveaway-scout-gpgv-") as verify_home:
            verify = ["gpgv", "--homedir", verify_home, "--keyring", str(public_key)]
            run([*verify, str(distribution / "InRelease")])
            run([*verify, str(distribution / "Release.gpg"), str(release)])
        (stage / "repository.json").write_text(json.dumps({
            "schemaVersion": 1, "suite": args.suite, "signingFingerprint": fingerprint,
            "epoch": epoch, "validUntilEpoch": epoch + args.valid_days * 86400,
            "architectures": architectures, "packages": packages,
            "nativeAcceptance": "not-proven-by-repository-preparation",
        }, ensure_ascii=False, indent=2) + "\n")
        # Every staged file is public metadata or a public package, never a key
        # secret. Make the finished snapshot servable even with a private umask.
        for path in stage.rglob("*"):
            path.chmod(0o755 if path.is_dir() else 0o644)
        stage.chmod(0o755)
        # Reserve a new directory atomically, then replace only our own empty
        # reservation. Existing repositories are never merged, erased or replaced.
        output.mkdir()
        try:
            os.replace(stage, output)
        except OSError:
            with suppress(OSError):
                output.rmdir()
            raise
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--deb", type=Path, action="append", required=True,
                        help="Giveaway Scout DEB; repeat for the other architecture")
    parser.add_argument("--suite", choices=SUITES, required=True)
    parser.add_argument("--output", type=Path, required=True, help="New local directory; must not exist")
    parser.add_argument("--gpg-home", type=Path, required=True, help="Existing isolated signing-key home")
    parser.add_argument("--signing-key", required=True, help="Full fingerprint of the exact signing key")
    parser.add_argument("--epoch", type=int, help="Release time; defaults to SOURCE_DATE_EPOCH or now")
    parser.add_argument("--valid-days", type=int, default=14)
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.exit(1, "APT preparation requires Linux (or a Linux environment in WSL).\n")
    try:
        output = prepare(args)
    except (ValueError, OSError, subprocess.SubprocessError, tarfile.TarError) as error:
        parser.exit(1, f"APT preparation failed: {error}\n")
    print(f"Prepared and verified local signed APT snapshot: {output}")
    print("Nothing was published, installed or added to the system APT configuration.")


if __name__ == "__main__":
    main()
