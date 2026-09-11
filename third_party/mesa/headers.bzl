"""Header-only GNU ABI inputs for the BexOS Mesa port.

No foreign libc implementation, CRT, package scripts, or linker scripts enter
the repository. BexOS supplies the implementation of every linked symbol.
"""

def _headers_impl(ctx):
    ctx.download(ctx.attr.url, "headers.deb", sha256 = ctx.attr.sha256)
    result = ctx.execute(["python3", ctx.path(ctx.attr._extract), ctx.path("headers.deb")])
    if result.return_code:
        fail("GNU header extraction failed: " + result.stderr)
    ctx.delete("headers.deb")
    ctx.file("BUILD.bazel", """
load("@rules_cc//cc:cc_library.bzl", "cc_library")
package(default_visibility = ["//visibility:public"])
cc_library(
    name = "headers",
    hdrs = glob(["usr/include/**"]),
    includes = ["usr/include"] + %s,
)
filegroup(name = "license", srcs = glob(["usr/share/doc/*/copyright"]))
""" % (repr(["usr/include/" + ctx.attr.triple]) if ctx.attr.triple else """select({
    "@platforms//cpu:aarch64": ["usr/include/aarch64-linux-gnu"],
    "@platforms//cpu:x86_64": ["usr/include/x86_64-linux-gnu"],
    "//conditions:default": [],
})"""))

gnu_headers = repository_rule(
    implementation = _headers_impl,
    attrs = {
        "url": attr.string(mandatory = True),
        "sha256": attr.string(mandatory = True),
        "triple": attr.string(),
        "_extract": attr.label(default = "//third_party/mesa:extract_headers.py"),
    },
)
