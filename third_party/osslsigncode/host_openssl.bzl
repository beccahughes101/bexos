"""Declare the host OpenSSL installation used by the EFI signing tool."""

def _impl(ctx):
    darwin = ctx.os.name == "mac os x"
    roots = ["/opt/homebrew/opt/openssl@3", "/usr/local/opt/openssl@3"] if darwin else ["/usr", "/usr/local"]
    root = None
    for candidate in roots:
        if ctx.path(candidate + "/include/openssl/opensslv.h").exists:
            root = candidate
            break
    if root == None:
        fail("EFI signing requires host OpenSSL development headers and libraries")
    ctx.symlink(root + "/include", "include")
    imports = []
    for name in ["crypto", "ssl"]:
        filename = "lib" + name + (".dylib" if darwin else ".so")
        directories = [root + "/lib"] if darwin else [root + "/lib", root + "/lib/x86_64-linux-gnu", root + "/lib/aarch64-linux-gnu", root + "/lib64"]
        found = None
        for directory in directories:
            if ctx.path(directory + "/" + filename).exists:
                found = directory + "/" + filename
                break
        if found == None:
            fail("Missing OpenSSL shared library: " + filename)
        ctx.symlink(found, filename)
        imports.append('cc_import(name = "%s", shared_library = "%s")' % (name, filename))
    ctx.file("BUILD.bazel", '\n'.join([
        'load("@rules_cc//cc:defs.bzl", "cc_import", "cc_library")',
        'package(default_visibility = ["//visibility:public"])',
    ] + imports + [
        'cc_library(name = "openssl", hdrs = glob(["include/openssl/**"]), includes = ["include"], deps = [":ssl", ":crypto"])',
    ]))

host_openssl = repository_rule(implementation = _impl, local = True, configure = True)
