"""Pinned native workflow validator, fetched only when the CI check needs it."""

_RELEASES = {
    ("linux", "amd64"): "8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8",
    ("linux", "arm64"): "325e971b6ba9bfa504672e29be93c24981eeb1c07576d730e9f7c8805afff0c6",
    ("darwin", "amd64"): "5b44c3bc2255115c9b69e30efc0fecdf498fdb63c5d58e17084fd5f16324c644",
    ("darwin", "arm64"): "aba9ced2dee8d27fecca3dc7feb1a7f9a52caefa1eb46f3271ea66b6e0e6953f",
}

def _actionlint_repository_impl(ctx):
    os = "darwin" if ctx.os.name == "mac os x" else ctx.os.name
    arch = {"aarch64": "arm64", "x86_64": "amd64"}.get(ctx.os.arch, ctx.os.arch)
    checksum = _RELEASES.get((os, arch))
    if not checksum:
        fail("actionlint requires a Linux or macOS x86_64/ARM64 execution host")
    ctx.download_and_extract(
        url = "https://github.com/rhysd/actionlint/releases/download/v1.7.12/actionlint_1.7.12_%s_%s.tar.gz" % (os, arch),
        sha256 = checksum,
    )
    ctx.file("BUILD.bazel", 'exports_files(["actionlint"], visibility = ["//visibility:public"])\n')

actionlint_repository = repository_rule(
    implementation = _actionlint_repository_impl,
    configure = True,
)
