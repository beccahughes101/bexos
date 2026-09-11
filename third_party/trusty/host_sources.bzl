"""Native host utilities built from the pinned Trusty superproject."""
load("@rules_cc//cc:defs.bzl", "cc_binary", "cc_library")

def trusty_native_tools():

    cc_library(
        name = "host_fmt",
        hdrs = native.glob(["external/fmtlib/include/fmt/**"]),
        includes = ["external/fmtlib/include"],
        defines = ["FMT_HEADER_ONLY"],
    )

    cc_library(
        name = "host_gtest",
        srcs = ["external/googletest/googletest/src/gtest-all.cc"],
        hdrs = native.glob(["external/googletest/googletest/include/**", "external/googletest/googletest/src/**"]),
        includes = ["external/googletest/googletest/include", "external/googletest/googletest"],
        linkopts = ["-pthread"],
    )

    cc_library(
        name = "host_base",
        copts = ["-std=c++20"] + select({
            "@platforms//os:linux": ["-D_POSIX_C_SOURCE=200809L"],
            "//conditions:default": [],
        }),
        srcs = ["system/libbase/" + name + ".cpp" for name in [
            "errors_unix", "file", "logging", "posix_strerror_r", "result", "stringprintf", "strings", "threads",
        ]],
        deps = [":host_base_headers", "@trusty_host_logging//:log"],
    )

    # Parser and lexer generation are declared Bazel actions, never checked in.
    native.genrule(
        name = "aidl_parser",
        srcs = ["system/tools/aidl/aidl_language_y.yy"],
        outs = ["aidl_parser/aidl_language_y.cpp", "aidl_parser/aidl_language_y.h", "aidl_parser/location.hh"],
        tools = ["@trusty_parser_tools//:bison", "@trusty_parser_tools//:bison_data"],
        cmd = "$(location @trusty_parser_tools//:bison) --defines=$(@D)/aidl_parser/aidl_language_y.h --output=$(@D)/aidl_parser/aidl_language_y.cpp $(SRCS)",
    )

    native.genrule(
        name = "aidl_lexer",
        srcs = ["system/tools/aidl/aidl_language_l.ll"],
        outs = ["aidl_parser/aidl_language_l.cpp"],
        tools = ["@trusty_parser_tools//:flex"],
        cmd = "$(location @trusty_parser_tools//:flex) -o $@ $(SRCS)",
    )

    cc_binary(
        name = "host_aidl",
        srcs = native.glob(["system/tools/aidl/*.cpp"], exclude = ["system/tools/aidl/*_unittest.cpp"]) + [":aidl_parser", ":aidl_lexer"],
        deps = [":host_aidl_headers", ":host_base", ":host_gtest"],
        copts = ["-std=c++20"],
        linkstatic = True,
    )
    cc_library(
        name = "host_aidl_headers",
        hdrs = native.glob(["system/tools/aidl/*.h", "system/tools/aidl/*.inc", "system/tools/aidl/include/**"]) + ["system/tools/aidl/hiddenapi-greylist"],
        includes = ["system/tools/aidl", "system/tools/aidl/include", "aidl_parser"],
    )

    native.genrule(
        name = "host_rpmb_main",
        srcs = ["trusty/user/app/storage/rpmb_dev/main.c", "@bexos//third_party/trusty:patch_rpmb_host.py"],
        outs = ["host_rpmb/main.c"],
        cmd = "python3 $(location @bexos//third_party/trusty:patch_rpmb_host.py) $(location trusty/user/app/storage/rpmb_dev/main.c) $@",
    )
    cc_binary(
        name = "host_rpmb_dev",
        srcs = ["trusty/user/app/storage/" + path for path in ["crypt.c", "rpmb_dev/rpmb_dev.c"]] + [":host_rpmb_main"],
        deps = [":host_rpmb_headers", "@secure_boot_host_openssl//:openssl"],
        copts = ["-Wno-deprecated-declarations"],
        defines = ["BUILD_STORAGE_TEST=1"],
        linkstatic = True,
    )
    cc_library(
        name = "host_rpmb_headers",
        hdrs = native.glob(["external/lk/include/shared/**", "trusty/user/app/storage/*.h", "trusty/user/app/storage/rpmb_dev/*.h"]),
        includes = ["external/lk/include/shared", "trusty/user/app/storage", "trusty/user/app/storage/rpmb_dev"],
    )

    cc_library(
        name = "host_base_headers",
        hdrs = native.glob(["system/libbase/include/**", "system/libbase/*.h"]),
        includes = ["system/libbase/include"],
        deps = [":host_fmt"],
    )
    cc_library(
        name = "host_cutils_headers",
        hdrs = native.glob(["system/core/libcutils/include/**", "system/core/libcutils/include_outside_system/**"]),
        includes = ["system/core/libcutils/include", "system/core/libcutils/include_outside_system"],
    )

    native.filegroup(
        name = "target_compatibility_sources",
        srcs = native.glob([
            "frameworks/native/libs/binder/rust/**/*.rs",
            "frameworks/native/libs/binder/liblog_stub/include/log/log.h",
            "trusty/user/base/lib/tipc/rust/src/**/*.rs",
            "trusty/user/base/lib/service_manager/client/src/*.rs",
            "trusty/user/app/authmgr/authmgr-fe/accessor.rs",
            "trusty/user/app/authmgr/authmgr-be/lib/src/authorization_service.rs",
            "trusty/user/app/sample/hwcryptohal/server/cmd_processing.rs",
        ]),
    )
