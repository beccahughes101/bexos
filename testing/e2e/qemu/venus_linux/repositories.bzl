"""Content-addressed Linux fixture dependencies from the reviewed prototxt lock."""
def _packages(ctx):
    archives = []
    record = None
    for line in ctx.read(ctx.attr.lock).splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if line == "archive {" and record == None:
            record = {}
        elif line == "}" and record != None:
            if sorted(record.keys()) != ["name", "sha256", "url", "version"]:
                fail("invalid fixture package fields")
            name = record["name"]
            if "/" in name or ".." in name or name in archives:
                fail("invalid or duplicate fixture package name")
            ctx.download(record["url"], "packages/" + name + ".apk", sha256 = record["sha256"])
            archives.append(name)
            record = None
        elif record != None:
            field, separator, value = line.partition(":")
            value = value.strip()
            if not separator or field in record or not value.startswith('"') or not value.endswith('"'):
                fail("invalid fixture package field")
            record[field] = value[1:-1]
        else:
            fail("invalid fixture package record")
    if record != None or not archives:
        fail("incomplete fixture package lock")
    ctx.file("BUILD.bazel", 'package(default_visibility=["//visibility:public"])\nfilegroup(name="archives",srcs=glob(["packages/*.apk"]))\n')

linux_packages = repository_rule(implementation = _packages, attrs = {"lock": attr.label(mandatory = True)})
