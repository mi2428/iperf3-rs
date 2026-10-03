"""Run with python3 scripts/check_native_rebuild.py; uses an isolated target fixture."""

import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import tempfile


def replace_once(path, before, after):
    text = path.read_text()
    assert text.count(before) == 1, (path, before)
    path.write_text(text.replace(before, after))


def main():
    root = Path(__file__).resolve().parents[1]
    target = root / "target"
    target.mkdir(exist_ok=True)
    compiler = shutil.which("cc")
    assert compiler, "a native C compiler is required"
    with tempfile.TemporaryDirectory(prefix="native-rebuild-", dir=target) as temporary:
        workspace = Path(temporary)
        fixture = workspace / "source"
        fixture.mkdir()
        for name in ("Cargo.toml", "Cargo.lock", "build.rs", "rust-toolchain.toml", "README.md"):
            shutil.copy2(root / name, fixture / name)
        ignore = shutil.ignore_patterns(".git", "target", ".libs", ".deps", "private.pem", "public.pem")
        for name in ("src", "native", "iperf3"):
            shutil.copytree(root / name, fixture / name, ignore=ignore)
        git_dir = workspace / "native-git"
        git_dir.mkdir()
        native_head = git_dir / "HEAD"
        native_head.write_text("0" * 40 + "\n")
        git_link = workspace / "native-git-link"
        git_link.write_text(f"gitdir: {git_dir}\n")
        shutil.copy2(git_link, fixture / "iperf3/.git")
        examples = fixture / "examples"
        examples.mkdir()
        (examples / "native_rebuild_probe.rs").write_text(
            'fn main() { println!("{}", iperf3_rs::libiperf_version()); }\n'
        )
        source = fixture / "iperf3/src/iperf_api.c"
        header = fixture / "iperf3/src/iperf_api.h"
        configure = fixture / "iperf3/configure"
        template = fixture / "iperf3/src/version.h.in"
        package_version = re.search(r"^PACKAGE_VERSION='([^']+)'", configure.read_text(), re.M).group(1)
        header.write_text(header.read_text() + "\n#define CACHE_HEADER 1\n"
                          "#ifndef CACHE_FLAG\n#define CACHE_FLAG 1\n#endif\n"
                          "#ifndef CACHE_CPP\n#define CACHE_CPP 1\n#endif\n"
                          "#ifndef CACHE_COMPILER\n#define CACHE_COMPILER 1\n#endif\n")
        replace_once(source, "static const char iperf_version[] = IPERF_VERSION;",
                     "#define CACHE_STRING_(x) #x\n#define CACHE_STRING(x) CACHE_STRING_(x)\n"
                     'static const char iperf_version[] = IPERF_VERSION " c=1 h=" '
                     'CACHE_STRING(CACHE_HEADER) " f=" CACHE_STRING(CACHE_FLAG) '
                     '" cpp=" CACHE_STRING(CACHE_CPP) '
                     '" cc=" CACHE_STRING(CACHE_COMPILER);')
        env = os.environ.copy()
        env.update(CC=compiler, CFLAGS="-O0", CPPFLAGS="", LDFLAGS="", LIBS="",
                   IPERF3_CONFIGURE_ARGS="--without-openssl", CARGO_TARGET_DIR=str(workspace / "build"))
        native_dir = None

        def build(expected):
            nonlocal native_dir
            subprocess.run(["cargo", "build", "--locked", "--offline", "--no-default-features",
                            "--example", "native_rebuild_probe"], cwd=fixture, env=env, check=True)
            output = Path(env["CARGO_TARGET_DIR"])
            current = list((output / "debug/build").glob("iperf3-rs-*/out/libiperf-build"))
            assert len(current) == 1, current
            if native_dir is not None:
                assert current[0] == native_dir, "regression must reuse the same OUT_DIR"
            native_dir = current[0]
            archive = native_dir / "src/.libs/libiperf.a"
            actual = subprocess.check_output([output / "debug/examples/native_rebuild_probe"],
                                             text=True).strip()
            assert actual == expected, (actual, expected)
            assert "#define HAVE_SSL 1" not in (native_dir / "src/iperf_config.h").read_text()
            return archive.stat().st_mtime_ns

        version = f"{package_version} c=1 h=1 f=1 cpp=1 cc=1"
        initial = build(version)
        assert build(version) == initial, "unchanged build should reuse the native archive"
        replace_once(source, '" c=1 h="', '" c=2 h="')
        version = version.replace(" c=1", " c=2")
        previous = initial
        for label, change in (
            ("C source", lambda: None),
            ("header", lambda: replace_once(header, "CACHE_HEADER 1", "CACHE_HEADER 2")),
            ("configure", lambda: replace_once(configure, f"PACKAGE_VERSION='{package_version}'", "PACKAGE_VERSION='cache-config'")),
            ("Autotools template", lambda: replace_once(template, '"@PACKAGE_VERSION@"', '"@PACKAGE_VERSION@-template"')),
            ("CFLAGS", lambda: env.update(CFLAGS="-O0 -DCACHE_FLAG=2")),
            ("CPPFLAGS", lambda: env.update(CPPFLAGS="-DCACHE_CPP=2")),
        ):
            change()
            if label == "header":
                version = version.replace("h=1", "h=2")
            elif label == "configure":
                version = version.replace(package_version, "cache-config")
            elif label == "Autotools template":
                version = version.replace("cache-config", "cache-config-template")
            elif label == "CFLAGS":
                version = version.replace("f=1", "f=2")
            elif label == "CPPFLAGS":
                version = version.replace("cpp=1", "cpp=2")
            rebuilt = build(version)
            assert rebuilt != previous, f"{label} did not rebuild the native archive"
            previous = rebuilt
            print(f"PASS warm {label}", flush=True)
        wrapper = workspace / "compiler"
        wrapper.write_text(f'#!/bin/sh\nexec {shlex.quote(compiler)} -DCACHE_COMPILER=2 "$@"\n')
        wrapper.chmod(0o755)
        env["CC"] = str(wrapper)
        version = version.replace("cc=1", "cc=2")
        assert build(version) != previous, "compiler change did not rebuild the native archive"
        print("PASS warm CC", flush=True)
        shutil.rmtree(fixture / "iperf3")
        shutil.copytree(root / "iperf3", fixture / "iperf3", ignore=ignore)
        shutil.copy2(git_link, fixture / "iperf3/.git")
        # Restored source mtimes are deliberately old; only the revision marker changes.
        native_head.write_text("1" * 40 + "\n")
        build(package_version)
        print("PASS refreshed vendored tree", flush=True)
        env["CARGO_TARGET_DIR"] = str(workspace / "fresh-build")
        native_dir = None
        build(package_version)
        print("PASS fresh/warm native version", flush=True)


if __name__ == "__main__":
    main()
