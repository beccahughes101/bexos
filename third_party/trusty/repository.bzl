def _trusty_repository_impl(ctx):
    git = ctx.which("git")
    if not git:
        fail("Trusty source setup requires git")
    result = ctx.execute([git, "clone", "--no-checkout", "https://android.googlesource.com/trusty/superproject", "."])
    if result.return_code:
        fail("clone Trusty superproject: %s" % result.stderr)
    result = ctx.execute([git, "checkout", "--detach", ctx.attr.commit])
    if result.return_code:
        fail("checkout Trusty superproject %s: %s" % (ctx.attr.commit, result.stderr))
    paths = [
        "external/arm-trusted-firmware", "external/boringssl", "external/dtc", "external/headers",
        "external/libcppbor", "external/libcxx", "external/libcxxabi", "external/lk",
        "external/fmtlib", "external/googletest",
        "external/open-dice", "external/python/jinja", "external/python/markupsafe",
        "external/python/six", "external/rust/android-crates-io", "prebuilts/build-tools",
        "external/rust/crates/openssl", "external/trusty/bootloader", "external/trusty/musl",
        "prebuilts/libprotobuf", "frameworks/hardware/interfaces", "frameworks/native",
        "hardware/interfaces", "hardware/libhardware", "system/authgraph",
        "packages/modules/Virtualization",
        "system/core", "system/gatekeeper",
        "system/libbase",
        "system/keymint", "system/secretkeeper", "system/see/authmgr", "system/tools/aidl",
        "trusty/device/arm/generic-arm64", "trusty/device/x86/generic-x86_64",
        "trusty/device/common", "trusty/interfaces", "trusty/kernel", "trusty/user/app/authmgr",
        "trusty/user/app/avb", "trusty/user/app/gatekeeper", "trusty/user/app/keymint",
        "trusty/host/common", "trusty/user/app/sample", "trusty/user/app/storage", "trusty/user/base",
        "trusty/vendor/google/aosp",
    ]
    result = ctx.execute([git, "submodule", "update", "--init", "--depth", "1"] + paths)
    if result.return_code:
        fail("initialize Trusty security-stack submodules: %s" % result.stderr)
    # This repository is consumed as source input by the upstream make build.
    # Nested Bazel package markers would otherwise stop the root glob at package
    # boundaries and silently omit trees such as BoringSSL's src directory.
    find = ctx.which("find")
    if not find:
        fail("Trusty source setup requires find")
    result = ctx.execute([find, ".", "-name", "BUILD.bazel", "-delete"])
    if result.return_code:
        fail("remove nested Trusty Bazel package markers: %s" % result.stderr)
    result = ctx.execute([find, ".", "-name", "BUILD", "-delete"])
    if result.return_code:
        fail("remove nested Trusty BUILD package markers: %s" % result.stderr)
    ctx.file("BUILD.bazel", 'load("%s", "trusty_native_tools")\ntrusty_native_tools()\n' % ctx.attr.host_build + """
package(default_visibility = [\"//visibility:public\"])
filegroup(name = \"all_sources\", srcs = glob([\"**\"], exclude = [\".git/**\"]))
""")
    ctx.file("external/dtc/BUILD.bazel", """
package(default_visibility = [\"//visibility:public\"])
filegroup(name = \"all_sources\", srcs = glob([\"**\"], exclude = [\".git/**\"]))
""")

trusty_repository = repository_rule(
    implementation = _trusty_repository_impl,
    attrs = {
        "commit": attr.string(mandatory = True),
        "host_build": attr.label(default = "//third_party/trusty:host_sources.bzl"),
    },
)

def _host_libclang_repository_impl(ctx):
    if ctx.os.name == "mac os x":
        xcrun = ctx.which("xcrun")
        if not xcrun:
            fail("Trusty bindgen requires Xcode command line tools (xcrun) on Darwin")
        result = ctx.execute([xcrun, "--find", "clang"])
        if result.return_code:
            fail("locate Xcode clang: %s" % result.stderr)
        candidates = [result.stdout.strip().rsplit("/", 2)[0] + "/lib/libclang.dylib"]
    else:
        candidates = []
        override = ctx.os.environ.get("LIBCLANG_PATH")
        if override:
            path = ctx.path(override)
            candidates = [str(path)] if not path.is_dir else [str(p) for p in path.readdir() if p.basename.startswith("libclang.so")]
        else:
            for tool in ["llvm-config"] + ["llvm-config-%d" % n for n in range(23, 13, -1)]:
                executable = ctx.which(tool)
                if executable:
                    result = ctx.execute([executable, "--libdir"])
                    if result.return_code == 0:
                        directory = ctx.path(result.stdout.strip())
                        candidates += [str(p) for p in directory.readdir() if p.basename.startswith("libclang.so")]
            for root in ["/usr/lib", "/usr/local/lib", "/usr/lib64", "/usr/lib/x86_64-linux-gnu", "/usr/lib/aarch64-linux-gnu"]:
                directory = ctx.path(root)
                if directory.exists:
                    candidates += [str(p) for p in directory.readdir() if p.basename.startswith("libclang.so")]
                    for child in directory.readdir():
                        if child.basename.startswith("llvm-") and child.get_child("lib").exists:
                            candidates += [str(p) for p in child.get_child("lib").readdir() if p.basename.startswith("libclang.so")]
    found = None
    for candidate in candidates:
        if ctx.path(candidate).exists:
            found = candidate
            break
    if found == None:
        fail("Trusty bindgen requires native libclang: install the LLVM libclang development package or set --repo_env=LIBCLANG_PATH to its library directory or file")
    filename = "lib/libclang.dylib" if ctx.os.name == "mac os x" else "lib/libclang.so"
    ctx.symlink(found, filename)
    ctx.file("BUILD.bazel", 'package(default_visibility = ["//visibility:public"])\nfilegroup(name = "libclang", srcs = ["%s"])\n' % filename)

host_libclang_repository = repository_rule(
    implementation = _host_libclang_repository_impl,
    environ = ["LIBCLANG_PATH"],
    local = True,
    configure = True,
)

def _host_macos_sdk_repository_impl(ctx):
    if ctx.os.name != "mac os x":
        # Queries traverse every select branch, including Darwin-only SDK labels
        # on Linux. Keep those labels loadable without probing for Xcode, while
        # preventing a build from using this repository as an actual SDK.
        ctx.file("BUILD.bazel", """
package(default_visibility = ["//visibility:public"])
[filegroup(
    name = name,
    target_compatible_with = ["@platforms//:incompatible"],
) for name in ["toolchain_files", "MacOSX.sdk/usr/include/stdio.h"]]
""")
        return
    xcrun = ctx.which("xcrun")
    if not xcrun:
        fail("Trusty RPMB proxy build requires xcrun on Darwin")
    result = ctx.execute([xcrun, "--sdk", "macosx", "--show-sdk-path"])
    if result.return_code:
        fail("locate macOS SDK: %s" % result.stderr)
    ctx.symlink(result.stdout.strip(), "MacOSX.sdk")
    ctx.file("BUILD.bazel", """
package(default_visibility = ["//visibility:public"])
exports_files(["MacOSX.sdk/usr/include/stdio.h"])
filegroup(
    name = "toolchain_files",
    srcs = glob([
        "MacOSX.sdk/usr/include/**",
        "MacOSX.sdk/usr/lib/**",
    ]),
)
""")

host_macos_sdk_repository = repository_rule(
    implementation = _host_macos_sdk_repository_impl,
    local = True,
)

def _host_parser_tools_impl(ctx):
    for name in ["bison", "flex"]:
        tool = ctx.which(name)
        if not tool:
            fail("Native Trusty AIDL generation requires %s; install the host bison and flex packages" % name)
        ctx.symlink(tool, name)
    result = ctx.execute([ctx.path("bison"), "--print-datadir"])
    if result.return_code:
        fail("locate Bison parser skeletons: " + result.stderr)
    ctx.symlink(result.stdout.strip(), "bison_data")
    ctx.file("BUILD.bazel", """
package(default_visibility = ["//visibility:public"])
exports_files(["bison", "flex"])
filegroup(name = "bison_data", srcs = glob(["bison_data/**"]))
""")

host_parser_tools = repository_rule(
    implementation = _host_parser_tools_impl,
    local = True,
    configure = True,
)
