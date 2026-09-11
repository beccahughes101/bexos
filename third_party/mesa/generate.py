#!/usr/bin/env python3
"""Run pinned Mesa generators inside a Bazel action; never edit vendored inputs."""
import argparse
import contextlib
import json
from pathlib import Path
import runpy
import sys


def main():
    parser = argparse.ArgumentParser()
    for name in ("mesa", "mako", "markupsafe", "pyyaml", "output"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args()
    mesa = Path(args.mesa).resolve().parent
    out = Path(args.output).resolve()
    out.mkdir(parents=True, exist_ok=True)
    # Release archives have no Git identity; use their pinned VERSION instead
    # of accidentally reading the enclosing BexOS checkout's commit.
    (out / "git_sha1.h").write_text('#define MESA_GIT_SHA1 ""\n#define PACKAGE_VERSION ' + json.dumps((mesa / "VERSION").read_text().strip()) + '\n')
    sys.dont_write_bytecode = True
    for root in (Path(args.mako).resolve().parent, Path(args.markupsafe).resolve().parent, Path(args.pyyaml).resolve().parent):
        sys.path[:0] = [str(root / "src"), str(root / "lib"), str(root)]
    util = mesa / "src/vulkan/util"
    sys.path.insert(0, str(util))
    sys.argv = [str(mesa / "src/util/driconf_static.py"), str(mesa / "src/util/00-mesa-defaults.conf"), str(out / "driconf_static.h")]
    runpy.run_path(sys.argv[0], run_name="__main__")
    xml = str(mesa / "src/vulkan/registry/vk.xml")

    def generate(script, stem, extra):
        script = util / script
        sys.argv = [str(script), "--xml", xml,
                    "--out-c", str(out / (stem + ".c")),
                    "--out-h", str(out / (stem + ".h")), *extra]
        # Generators use argparse and may raise SystemExit(0).
        try:
            runpy.run_path(str(script), run_name="__main__")
        except SystemExit as error:
            if error.code not in (None, 0):
                raise

    generate("vk_entrypoints_gen.py", "vn_entrypoints",
             ["--proto", "--weak", "--prefix", "vn", "--beta", "false"])
    generate("vk_dispatch_table_gen.py", "vk_dispatch_table", ["--beta", "false"])
    generate("vk_extensions_gen.py", "vk_extensions", [])
    generate("gen_enum_to_str.py", "vk_enum_to_str",
             ["--out-d", str(out / "vk_enum_defines.h"), "--beta", "false"])
    generate("vk_entrypoints_gen.py", "vk_common_entrypoints",
             ["--proto", "--weak", "--prefix", "vk_common", "--beta", "false"])
    for name in ("vk_cmd_queue", "vk_physical_device_features", "vk_physical_device_properties", "vk_dispatch_trampolines"):
        generate(name + "_gen.py", name, ["--beta", "false"])
    generate("../runtime/vk_format_info_gen.py", "vk_format_info", [])
    generate("vk_entrypoints_gen.py", "vk_cmd_enqueue_entrypoints",
             ["--proto", "--weak", "--prefix", "vk_cmd_enqueue", "--prefix", "vk_cmd_enqueue_unless_primary", "--beta", "false"])
    for name in ("vk_physical_device_spirv_caps", "vk_synchronization_helpers"):
        sys.argv = [str(util / (name + "_gen.py")), "--xml", xml,
                    "--out-c", str(out / (name + ".c")), "--beta", "false"]
        runpy.run_path(sys.argv[0], run_name="__main__")
    sys.argv = [str(util / "vk_struct_type_cast_gen.py"), "--xml", xml,
                "--out", str(out / "vk_struct_type_cast.h"), "--beta", "false"]
    runpy.run_path(sys.argv[0], run_name="__main__")
    formats = mesa / "src/util/format"
    sys.path.insert(0, str(formats))
    (out / "util/format").mkdir(parents=True, exist_ok=True)
    sys.argv = [str(formats / "u_format_table.py"), str(formats / "u_format.yaml"), "--enums"]
    with (out / "util/format/u_format_gen.h").open("w") as header, contextlib.redirect_stdout(header):
        runpy.run_path(sys.argv[0], run_name="__main__")
    for name, flags in (("util/format/u_format_pack.h", ["--header"]), ("u_format_table.c", [])):
        sys.argv = [str(formats / "u_format_table.py"), str(formats / "u_format.yaml"), *flags]
        with (out / name).open("w") as generated, contextlib.redirect_stdout(generated):
            runpy.run_path(sys.argv[0], run_name="__main__")
    sys.argv = [str(mesa / "src/util/format_srgb.py")]
    with (out / "format_srgb.c").open("w") as generated, contextlib.redirect_stdout(generated):
        runpy.run_path(sys.argv[0], run_name="__main__")
    spirv = mesa / "src/compiler/spirv"
    (out / "compiler/spirv").mkdir(parents=True, exist_ok=True)
    sys.argv = [str(spirv / "spirv_info_gen.py"), "--json", str(spirv / "spirv.core.grammar.json"),
                "--out-h", str(out / "compiler/spirv/spirv_info.h"), "--out-c", str(out / "spirv_info.c")]
    runpy.run_path(sys.argv[0], run_name="__main__")


if __name__ == "__main__":
    main()
