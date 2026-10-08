#!/usr/bin/env python3
"""Build the pinned controlled Codex fork; never modify an installed Codex."""
import argparse
import hashlib
import json
import os
import subprocess
import tarfile
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent


def prepare(archive, destination):
    metadata = json.loads((HERE / "source.json").read_text())
    if hashlib.sha256(archive.read_bytes()).hexdigest() != metadata["archive_sha256"]:
        raise ValueError("Codex archive digest does not match the pinned source")
    source = destination / ("codex-" + metadata["revision"])
    if source.exists():
        raise ValueError("Use a fresh work directory; existing sources are never reset")
    with tarfile.open(archive) as stream:
        stream.extractall(destination, filter="data")
    for name, expected in metadata["original_files"].items():
        path = source / name
        if expected is None:
            if path.exists():
                raise ValueError("New controlled source file already exists: " + name)
        elif hashlib.sha256(path.read_bytes()).hexdigest() != expected:
            raise ValueError("Codex source file digest mismatch: " + name)
    subprocess.run(["git", "apply", "--check", str(HERE / "stateless-responses.patch")], cwd=source, check=True)
    subprocess.run(["git", "apply", str(HERE / "stateless-responses.patch")], cwd=source, check=True)
    return source


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--prepare-only", action="store_true")
    args = parser.parse_args()
    args.work_dir.mkdir(parents=True, exist_ok=True)
    archive = args.archive
    if archive is None:
        metadata = json.loads((HERE / "source.json").read_text())
        archive = args.work_dir / "source.tar.gz"
        with urllib.request.urlopen("https://codeload.github.com/openai/codex/tar.gz/" + metadata["revision"], timeout=30) as response, archive.open("xb") as output:
            size = 0
            while chunk := response.read(1024 * 1024):
                size += len(chunk)
                if size > 128 * 1024 * 1024:
                    raise ValueError("Codex archive exceeds the size limit")
                output.write(chunk)
    source = prepare(archive, args.work_dir)
    if args.prepare_only:
        print(source)
        return
    subprocess.run(["cargo", "+1.98.1", "test", "--locked", "-p", "codex-app-server", "--lib", "llmproxy_proxy_tests"], cwd=source / "codex-rs", check=True, env={**os.environ, "LLMPROXY_CODEX_PROXY_V1": "1"})
    subprocess.run(["cargo", "+1.98.1", "build", "--locked", "--release", "-p", "codex-cli", "--bin", "codex"], cwd=source / "codex-rs", check=True)
    print(source / "codex-rs/target/release/codex")


if __name__ == "__main__":
    main()
