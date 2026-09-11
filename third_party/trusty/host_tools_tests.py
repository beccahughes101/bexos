"""Exercise native tools and both guest ABI patches against pinned sources."""
import ctypes
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile
import unittest

RUSTC, RUSTFMT, LIBCLANG, AIDL, RPMB, ENVIRONMENT, COMPAT, SOURCE_MARKER, PREREQUISITES, REVERSE = map(
    lambda arg: Path(arg).resolve(), sys.argv[1:]
)
SOURCE = Path(sys.argv[8]).absolute().parent
sys.argv[1:] = []


def run(*args):
    return subprocess.check_output(list(map(str, args)), text=True, stderr=subprocess.STDOUT)


class HostToolsTests(unittest.TestCase):
    def test_dependency_order_reversal(self):
        result = subprocess.run([sys.executable, str(REVERSE)], input=b"core\nalloc\nstd\n",
                                capture_output=True, check=True)
        self.assertEqual(result.stdout, b"std\nalloc\ncore\n")
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "dependencies"
            source.write_text("one\ntwo\n")
            self.assertEqual(run(sys.executable, REVERSE, source), "two\none\n")

    def test_missing_host_dependency_diagnostic(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ["make", "python3", "xxd", "sed"]:
                path = root / name
                path.write_text("#!/bin/sh\nexit 0\n")
                path.chmod(0o755)
            result = subprocess.run(["/bin/bash", "-c", '. "$1"', "test", str(PREREQUISITES)],
                                    env={**os.environ, "PATH": str(root)},
                                    capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Trusty requires dtc on the execution host", result.stderr)

    def test_native_compilers_and_sdk_selection(self):
        host = {("Linux", "x86_64"): "x86_64-unknown-linux-gnu",
                ("Linux", "aarch64"): "aarch64-unknown-linux-gnu",
                ("Darwin", "arm64"): "aarch64-apple-darwin"}[
                    platform.system(), platform.machine()]
        self.assertIn("rustc 1.80.1", run(RUSTC, "--version"))
        self.assertIn("host: " + host, run(RUSTC, "-vV"))
        self.assertIn("rustfmt", run(RUSTFMT, "--version"))
        self.assertTrue(hasattr(ctypes.CDLL(str(LIBCLANG)), "clang_getClangVersion"))
        sdk = run("bash", "-c", '. "$1"; printf "%s" "$TRUSTY_MACOS_SDK_MARKER"',
                  "host-test", ENVIRONMENT)
        if platform.system() == "Darwin":
            self.assertIn("MacOSX.sdk/usr/include/stdio.h", sdk)
        else:
            self.assertEqual(sdk, "")

    def test_aidl_generates_rust_and_cpp_natively(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            aidl = root / "bexos/host/IHost.aidl"
            aidl.parent.mkdir(parents=True)
            aidl.write_text("package bexos.host; interface IHost { int ping(int value); }\n")
            for language, suffix in [("rust", ".rs"), ("cpp", ".cpp")]:
                out = root / language
                out.mkdir()
                headers = ["-h" + str(out)] if language == "cpp" else []
                run(AIDL, "--lang=" + language, "-I" + str(root), "-o" + str(out),
                    *headers, aidl)
                generated = list(out.rglob("*" + suffix))
                self.assertTrue(generated, language)
                self.assertTrue(any("ping" in path.read_text() for path in generated))

    def test_native_rpmb_initialization(self):
        with tempfile.TemporaryDirectory() as directory:
            data = Path(directory) / "RPMB_DATA"
            run(RPMB, "--dev", data, "--init", "--size", "2048")
            self.assertGreater(data.stat().st_size, 0)

    def test_target_compatibility_on_both_guests(self):
        # Copy only the files touched by the compatibility helper, preserving
        # real upstream syntax rather than teaching the test replacement text.
        paths = [
            "frameworks/native/libs/binder/liblog_stub/include/log/log.h",
            "frameworks/native/libs/binder/rust/src/binder.rs",
            "frameworks/native/libs/binder/rust/src/proxy.rs",
            "frameworks/native/libs/binder/rust/src/parcel/file_descriptor.rs",
            "frameworks/native/libs/binder/rust/src/lib.rs",
            "frameworks/native/libs/binder/rust/src/native.rs",
            "frameworks/native/libs/binder/rust/src/error.rs",
            "frameworks/native/libs/binder/rust/src/parcel/parcelable.rs",
            "trusty/user/base/lib/tipc/rust/src/handle.rs",
            "trusty/user/base/lib/tipc/rust/src/raw/handle_set_wrapper.rs",
            "trusty/user/base/lib/tipc/rust/src/service.rs",
            "trusty/user/base/lib/service_manager/client/src/lib.rs",
            "frameworks/native/libs/binder/rust/rpcbinder/src/session.rs",
            "frameworks/native/libs/binder/rust/rpcbinder/src/server/trusty.rs",
            "trusty/user/app/authmgr/authmgr-fe/accessor.rs",
            "trusty/user/app/authmgr/authmgr-be/lib/src/authorization_service.rs",
            "trusty/user/app/sample/hwcryptohal/server/cmd_processing.rs",
        ]
        sed = shutil.which("gsed") or shutil.which("sed")
        for architecture, char in [("aarch64", "u8"), ("x86_64", "i8")]:
            with self.subTest(architecture=architecture), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                for path in paths:
                    target = root / path
                    target.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(SOURCE / path, target)
                run("bash", COMPAT, root, sed, architecture)
                for path in paths:
                    text = (root / path).read_text()
                    # Non-Trusty function bodies retain their platform imports.
                    self.assertEqual([line for line in text.splitlines()
                                      if line.startswith("use std::os::fd::")], [], path)
                binder = (root / paths[1]).read_text()
                self.assertIn("type BinderChar = " + char + ";", binder)
                self.assertIn("pub mod fd_compat;", (root / paths[4]).read_text())
                self.assertIn("pub fn into_raw_fd", (root / paths[8]).read_text())


if __name__ == "__main__":
    unittest.main()
