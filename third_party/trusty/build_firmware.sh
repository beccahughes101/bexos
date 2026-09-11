#!/bin/bash
set -eu

src_root="$1"
overlay_root="$2"
out_root="$3"
clang_bin="$4"
clangxx_bin="$5"
ld_lld="$6"
llvm_ar="$7"
llvm_cxxfilt="$8"
llvm_nm="$9"
llvm_objcopy="${10}"
llvm_objdump="${11}"
llvm_ranlib="${12}"
llvm_readelf="${13}"
llvm_size="${14}"
llvm_strip="${15}"
protoc_tool="${16}"
bindgen_tool="${17}"
libclang_tool="${18}"
rustc_tool="${19}"
rustfmt_tool="${20}"
rust_src_marker="${21}"
bl33_verifier="${22}"
avb_root_key="${23}"
compiler_rt_marker="${24}"
mbedtls_version_marker="${25}"
rustdoc_tool="${26}"
rust_std_patch="${27}"
tf_a_memory_patch="${28}"
macos_sdk_marker="${29}"
rpmb_tool="${30}"
aidl_tool="${31}"
compatibility_script="${32}"
prerequisites_script="${33}"
reverse_lines_tool="${34}"
reverse_lines_tool="$(cd "$(dirname "$reverse_lines_tool")" && pwd)/$(basename "$reverse_lines_tool")"
architecture="${BEXOS_TRUSTY_ARCH:-aarch64}"
rollback_verifier="${BEXOS_TRUSTY_ROLLBACK_VERIFIER:-}"
if [ "$architecture" = aarch64 ]; then
  test -n "$rollback_verifier" || { echo "missing signed rollback fixture input" >&2; exit 1; }
  rollback_verifier="$(cd "$(dirname "$rollback_verifier")" && pwd)/$(basename "$rollback_verifier")"
fi
x86_config="${BEXOS_TRUSTY_X86_CONFIG:-}"
if [ -n "$x86_config" ]; then x86_config="$(cd "$(dirname "$x86_config")" && pwd)/$(basename "$x86_config")"; fi

out_root="$(mkdir -p "$out_root" && cd "$out_root" && pwd)"
src_root="$(cd "$src_root" && pwd)"
overlay_root="$(cd "$overlay_root" && pwd)"
clang_bin="$(cd "$(dirname "$clang_bin")" && pwd)/$(basename "$clang_bin")"
clangxx_bin="$(cd "$(dirname "$clangxx_bin")" && pwd)/$(basename "$clangxx_bin")"
ld_lld="$(cd "$(dirname "$ld_lld")" && pwd)/$(basename "$ld_lld")"
llvm_ar="$(cd "$(dirname "$llvm_ar")" && pwd)/$(basename "$llvm_ar")"
llvm_cxxfilt="$(cd "$(dirname "$llvm_cxxfilt")" && pwd)/$(basename "$llvm_cxxfilt")"
llvm_nm="$(cd "$(dirname "$llvm_nm")" && pwd)/$(basename "$llvm_nm")"
llvm_objcopy="$(cd "$(dirname "$llvm_objcopy")" && pwd)/$(basename "$llvm_objcopy")"
llvm_objdump="$(cd "$(dirname "$llvm_objdump")" && pwd)/$(basename "$llvm_objdump")"
llvm_ranlib="$(cd "$(dirname "$llvm_ranlib")" && pwd)/$(basename "$llvm_ranlib")"
llvm_readelf="$(cd "$(dirname "$llvm_readelf")" && pwd)/$(basename "$llvm_readelf")"
llvm_size="$(cd "$(dirname "$llvm_size")" && pwd)/$(basename "$llvm_size")"
llvm_strip="$(cd "$(dirname "$llvm_strip")" && pwd)/$(basename "$llvm_strip")"
protoc_tool="$(cd "$(dirname "$protoc_tool")" && pwd)/$(basename "$protoc_tool")"
bindgen_tool="$(cd "$(dirname "$bindgen_tool")" && pwd)/$(basename "$bindgen_tool")"
libclang_dir="$(cd "$(dirname "$libclang_tool")" && pwd)"
rustc_tool="$(cd "$(dirname "$rustc_tool")" && pwd)/$(basename "$rustc_tool")"
rustfmt_tool="$(cd "$(dirname "$rustfmt_tool")" && pwd)/$(basename "$rustfmt_tool")"
rustdoc_tool="$(cd "$(dirname "$rustdoc_tool")" && pwd)/$(basename "$rustdoc_tool")"
rust_std_patch="$(cd "$(dirname "$rust_std_patch")" && pwd)/$(basename "$rust_std_patch")"
tf_a_memory_patch="$(cd "$(dirname "$tf_a_memory_patch")" && pwd)/$(basename "$tf_a_memory_patch")"
macos_sdk_root=
if [ -n "$macos_sdk_marker" ]; then
  macos_sdk_root="$(cd "$(dirname "$macos_sdk_marker")/../.." && pwd)"
  export SDKROOT="$macos_sdk_root"
fi
rpmb_tool="$(cd "$(dirname "$rpmb_tool")" && pwd)/$(basename "$rpmb_tool")"
aidl_tool="$(cd "$(dirname "$aidl_tool")" && pwd)/$(basename "$aidl_tool")"
compatibility_script="$(cd "$(dirname "$compatibility_script")" && pwd)/$(basename "$compatibility_script")"
rust_src_root="$(cd "$(dirname "$rust_src_marker")/../../.." && pwd)"
bl33_verifier="$(cd "$(dirname "$bl33_verifier")" && pwd)/$(basename "$bl33_verifier")"
avb_root_key="$(cd "$(dirname "$avb_root_key")" && pwd)/$(basename "$avb_root_key")"
mbedtls_root="$(cd "$(dirname "$mbedtls_version_marker")/../.." && pwd)"
rust_host_libdir="$("$rustc_tool" --print target-libdir)"

. "$prerequisites_script"
openssl_dir=
if [ "$(uname -s)" = "Darwin" ]; then
  openssl_bin="$(command -v openssl || true)"
  test -n "$openssl_bin" || {
    echo "TF-A certificate generation requires OpenSSL on Darwin" >&2
    exit 1
  }
  package_prefix="$(cd "$(dirname "$openssl_bin")/.." && pwd)"
  openssl_dir="$package_prefix/opt/openssl@3"
  test -f "$openssl_dir/include/openssl/conf.h" || {
    echo "OpenSSL headers are unavailable at $openssl_dir" >&2
    exit 1
  }
fi
# Rust 1.80 predates the upstream Trusty target's C ABI and single-threaded TLS
# selections. Apply the version-scoped compatibility patch only to the staged
# standard library used by this firmware action.
patch -d "$rust_src_root" -p1 < "$rust_std_patch"

# Proc macros link against the native host runtime, never Android's x86
# sysroot. Darwin needs Apple's linker; Linux uses the declared LLVM driver.
host_rust_linker="$clangxx_bin"
if [ "$(uname -s)" = "Darwin" ]; then host_rust_linker=/usr/bin/clang++; fi
engine_mk="$src_root/external/lk/engine.mk"
"$sed_cmd" -i \
  '/^GLOBAL_HOST_RUST_LINK_ARGS :=/,/^GLOBAL_HOST_RUSTFLAGS += -C linker=/c\
GLOBAL_HOST_RUST_LINK_ARGS :=\
GLOBAL_HOST_RUSTFLAGS += -C linker="'"$host_rust_linker"'"' \
  "$engine_mk"

work="$out_root/work"
rm -rf "$work"
rm -rf "$out_root/build-root"
mkdir -p "$work"
mv "$src_root/"* "$work/"
patch -d "$work" -p1 < "$tf_a_memory_patch"
mkdir -p "$work/trusty/user/app/bexos/orchestrator"
cp -R "$overlay_root/." "$work/trusty/user/app/bexos/orchestrator/"

# The pinned AuthMgr reference application still selects its in-memory test
# backend. Replace only that vendor seam with a bounded adapter over Trusty's
# rollback-protected persistent storage service.
authmgr_be_lib="$work/trusty/user/app/authmgr/authmgr-be/lib"
# coset's detached verification asserts that no embedded payload is present.
# Treat malformed peer input as an authentication error before reaching it.
"$sed_cmd" -i '/let cose_sign1 = CoseSign1::from_slice(signature)?;/a\
        if cose_sign1.payload.is_some() {\
            return Err(amc_err!(SignatureVerificationFailed, "embedded connection request payload"));\
        }' "$work/system/see/authmgr/authmgr-common/src/signed_connection_request.rs"
"$sed_cmd" -i \
  -e '/mod authorization_service;/a\
mod trusty_storage;' \
  "$authmgr_be_lib/src/lib.rs"
"$sed_cmd" -i \
  -e '/mod authorization_service;/a\
mod authmgr_storage_format;' \
  "$authmgr_be_lib/src/lib.rs"
"$sed_cmd" -i \
  -e 's/use authmgr_be_impl::mock_storage::MockPersistentStorage;/use crate::trusty_storage::TrustyPersistentStorage;/' \
  -e 's/Box::new(MockPersistentStorage::new())/Box::new(TrustyPersistentStorage::new()?)/' \
  "$authmgr_be_lib/src/authorization_service.rs"
"$sed_cmd" -i \
  '/trusty\/user\/base\/lib\/trusty-std/a\
\ttrusty/user/base/lib/storage/rust \\' \
  "$authmgr_be_lib/rules.mk"

# AuthMgr BE depends on the storage TA's runtime-created client ports. On SMP,
# an eager built-in AuthMgr thread can run before the boot CPU has even created
# the later, alphabetically ordered storage task. Register AuthMgr's real port
# as a deferred-start port so the first authorized connection starts it after
# the secure runtime is online; the kernel enforces the TA/NS access flags.
authmgr_be_manifest="$work/trusty/user/app/authmgr/authmgr-be/app/manifest.json"
"$sed_cmd" -i \
  's/"min_stack": 32768/"min_stack": 32768,\n    "mgmt_flags": {\n        "deferred_start": true\n    },\n    "start_ports": [\n        {\n            "name": "com.android.trusty.rust.authmgr.V1",\n            "flags": {\n                "allow_ta_connect": true,\n                "allow_ns_connect": true\n            }\n        }\n    ]/' \
  "$authmgr_be_manifest"

# The FE code is staged as a focused source overlay. Keep its bounded runtime
# stack aligned with the library manifest.
authmgr_fe_manifest="$work/trusty/user/app/authmgr/authmgr-fe/app/manifest.json"
"$sed_cmd" -i 's/"min_stack": 8192/"min_stack": 16384/' "$authmgr_fe_manifest"

# Built-in tasks are canonicalized by upstream make, so source declaration
# order cannot ensure storage has published its dynamically-created client
# ports before storage-backed TAs connect. Run the storage service at Trusty's
# HIGH_PRIORITY (24); once it blocks in its event loop, the ordinary-priority
# KeyMint, Gatekeeper, AVB, and AuthMgr clients can initialize deterministically.
storage_manifest="$work/trusty/user/app/storage/manifest.json"
"$sed_cmd" -i \
  's/"min_stack": 16384,/"min_stack": 16384,\n    "priority": 24,/' \
  "$storage_manifest"

# TF-A checks the declared Mbed TLS major/minor with GNU grep's Perl-regex
# option. BSD grep has no -P, so read the exact same version macros with awk in
# the action-local copy. Keep the major-version guard itself intact.
mbedtls_common_mk="$work/external/arm-trusted-firmware/drivers/auth/mbedtls/mbedtls_common.mk"
"$sed_cmd" -i \
  -e 's|^MBEDTLS_MAJOR=.*|MBEDTLS_MAJOR=$(shell awk '\''/define MBEDTLS_VERSION_MAJOR/ { print $$3; exit }'\'' ${MBEDTLS_DIR}/include/mbedtls/*.h)|' \
  -e 's|^MBEDTLS_MINOR=.*|MBEDTLS_MINOR=$(shell awk '\''/define MBEDTLS_VERSION_MINOR/ { print $$3; exit }'\'' ${MBEDTLS_DIR}/include/mbedtls/*.h)|' \
  "$mbedtls_common_mk"

# The pinned zerocopy revision uses core::error::Error, which remains gated in
# the pinned Trusty Rust 1.80 compiler. The wrapper already sets
# RUSTC_BOOTSTRAP=1 for upstream's nightly features, so enable this matching
# feature in the action-local copy until the pinned compiler is advanced.
zerocopy_lib="$work/external/rust/android-crates-io/crates/zerocopy/src/lib.rs"
"$sed_cmd" -i '/#!\[allow(unknown_lints/i#![feature(error_in_core)]' "$zerocopy_lib"

# The pinned BoringSSL revision uses a small set of libc++ headers, but its
# Trusty rules do not declare that private compile-time include dependency.
# Keep the fix scoped to BoringSSL: adding libc++ globally shadows musl's C
# compatibility headers in unrelated kernel modules. Trusty's BoringSSL build
# uses the thread API already selected by Trusty's user-space C++ runtime.
boringssl_rules="$work/external/boringssl/rules.mk"
"$sed_cmd" -i '/MODULE_COMPILEFLAGS += $(MODULE_STATIC_ARMCAP)/a\
MODULE_COMPILEFLAGS += -Iexternal/libcxx/include' \
  "$boringssl_rules"

bash "$compatibility_script" "$work" "$sed_cmd" "$architecture"

host_proc_macro_ext=so
if [ "$(uname -s)" = "Darwin" ]; then
  host_proc_macro_ext=dylib
  rust_mk="$work/external/lk/make/rust.mk"
  module_mk="$work/external/lk/make/module.mk"
  rust_toplevel_mk="$work/external/lk/make/rust-toplevel.mk"

  userspace_library_mk="$work/trusty/user/base/make/library.mk"
  trusted_app_mk="$work/trusty/user/base/make/trusted_app.mk"
  "$sed_cmd" -i \
    's|lib$(MODULE_RUST_STEM)\.so|lib$(MODULE_RUST_STEM).dylib|g' \
    "$rust_mk"
  "$sed_cmd" -i \
    -e 's|lib$(stem)\.so|lib$(stem).dylib|g' \
    -e 's|lib$(2)\.so|lib$(2).dylib|g' \
    "$module_mk" "$rust_toplevel_mk"
  "$sed_cmd" -i \
    -e 's|lib$(MODULE_RUST_STEM)\.so|lib$(MODULE_RUST_STEM).dylib|g' \
    -e 's|%.rlib %.so|%.rlib %.so %.dylib|g' \
    "$userspace_library_mk"
  # Proc macros are host executables used while rustc expands the target
  # crate. Trusty's app linker excludes Linux .so host tools from target ELF
  # arguments; apply the equivalent exclusion to Darwin .dylib artifacts.
  "$sed_cmd" -i \
    's|%.rlib %.so|%.rlib %.so %.dylib|g' \
    "$trusted_app_mk"
fi

# Upstream's make status helpers print every recursively expanded dependency
# and command. That exceeds Bazel's retained action log for this full security
# stack and can hide the diagnostic that actually stopped the build. Keep the
# copied firmware action concise while preserving compiler and linker stderr.
make_macros="$work/external/lk/make/macros.mk"
"$sed_cmd" -i \
  -e 's/^INFO = .*/INFO =/' \
  -e 's/^INFO_DONE = .*/INFO_DONE =/' \
  -e 's/^INFO_DONE_SILENT = .*/INFO_DONE_SILENT =/' \
  -e 's/^ECHO = .*/ECHO = true/' \
  -e 's/^ECHO_DONE = .*/ECHO_DONE = true/' \
  -e 's/^ECHO_DONE_SILENT = .*/ECHO_DONE_SILENT = true/' \
  "$make_macros"

# Both firmware variants need room for runtime page tables and secure storage
# buffers in QEMU's fixed secure RAM arena. Retain debug assertions but omit
# embedded symbol tables and enable inlining and size optimization.
"$sed_cmd" -i 's/ARCH_OPTFLAGS := -O2/ARCH_OPTFLAGS := -Oz/' \
  "$work/external/lk/arch/arm64/rules.mk"

# The pinned HWBCC provider predates AuthMgr FE. Permit only that secure TA
# identity to request its certificate chain and sign the FE/BE handshake.
# The kernel supplies the peer UUID; normal-world callers cannot impersonate it.
"$sed_cmd" -i '/^static const struct uuid\* allowed_uuids\[\] = {/i\
static const struct uuid authmgr_fe_uuid = {\
    0x9b3c1e9e, 0x1808, 0x4b98,\
    {0x8f, 0xa9, 0x85, 0x92, 0xdf, 0xf3, 0xa3, 0x37},\
};' "$work/trusty/user/base/lib/hwbcc/srv/srv.c"
"$sed_cmd" -i '/^static const struct uuid\* allowed_uuids\[\] = {/a\
        \&authmgr_fe_uuid,' "$work/trusty/user/base/lib/hwbcc/srv/srv.c"

cat > "$work/trusty/device/arm/generic-arm64/project/bexos-trusty-qemu-debug.mk" <<'EOF'
SPMC_EL :=
LIB_SM_WITH_FFA_LOOP := false
SMP_MAX_CPUS := 4

KERNEL_32BIT := false
DEBUG := 2
RELEASE_BUILD := false
KERNEL_CFI_ENABLED := false
USER_CFI_ENABLED := false

SYMTAB_ENABLED := false
include project/generic-arm64-debug.mk
USERSPACE_INLINE_FUNCTIONS := true
KERNEL_INLINE_FUNCTIONS := true
GLOBAL_SHARED_COMPILEFLAGS += -Wno-unused-template -Wno-nontrivial-memcall -fsigned-char -Iexternal/boringssl/src/include

TRUSTY_PREBUILT_USER_TASKS :=
TRUSTY_BUILTIN_USER_TASKS := \
	trusty/user/app/sample/hwcrypto \
	trusty/user/app/sample/hwbcc \
	trusty/user/app/sample/hwcryptohal/server/app \
	trusty/user/base/app/system_state_server_static \
	trusty/user/app/storage \
	trusty/user/app/keymint/app \
	trusty/user/app/gatekeeper \
	trusty/user/app/avb \
	trusty/user/app/authmgr/authmgr-fe/app \
	trusty/user/app/authmgr/authmgr-be/app \
	trusty/user/app/bexos/orchestrator \

TRUSTY_ALL_USER_TASKS := $(TRUSTY_BUILTIN_USER_TASKS)
LK_BIN := $(BUILDDIR)/lk.bin
BL32_BIN := $(LK_BIN)

# Match upstream's QEMU storage contract without importing qemu-inc.mk (which
# recursively selects a different project and test-runner BL33). The secure
# storage service derives its development RPMB key and the host proxy is built
# as part of the same pinned firmware action.
WITH_HKDF_RPMB_KEY := true
# This image has authenticated RPMB storage without a nonsecure file backend.
# Upstream Gatekeeper otherwise waits forever for the absent TD filesystem.
GATEKEEPER_STORAGE_PORT := STORAGE_CLIENT_TP_PORT
# Match upstream qemu-inc.mk: a pristine emulator RPMB is provisioned by the
# storage TA on its first authenticated connection, using its derived key.
STATIC_SYSTEM_STATE_FLAG_PROVISIONING_ALLOWED := 1
# The QEMU image intentionally does not ship the optional
# metrics consumer. Keep secure storage available without making telemetry a
# startup dependency; RPMB error paths still fail closed locally.
STORAGE_ENABLE_ERROR_REPORTING := false
MODULES += trusty/user/app/storage/rpmb_dev
RPMB_DEV := $(BUILDDIR)/host_tools/rpmb_dev

ATF_DEBUG := 1
ATF_PLAT := qemu
ATF_WITH_TRUSTY_GENERIC_SERVICES := true
ATF_BUILD_BASE := $(abspath $(BUILDDIR)/atf)
ATF_TOOLCHAIN_PREFIX := $(ARCH_arm64_TOOLCHAIN_PREFIX)
ATF_ROOT := $(call FIND_EXTERNAL,arm-trusted-firmware)
include project/qemu-atf-inc.mk
GLOBAL_SHARED_COMPILEFLAGS += -Wno-nontrivial-memcall -fsigned-char -Iexternal/boringssl/src/include
EOF

# The RPMB helper is built natively by Bazel on every host, outside upstream's
# x86-only host sysroot. It initializes the output template after firmware make.
project_mk="$work/trusty/device/arm/generic-arm64/project/bexos-trusty-qemu-debug.mk"
"$sed_cmd" -i \
  -e '/^MODULES += trusty\/user\/app\/storage\/rpmb_dev$/d' \
  -e '/^RPMB_DEV := .*host_tools\/rpmb_dev$/d' \
  "$project_mk"

# These providers include RNG headers directly; declare the library at each
# boundary instead of relying on transitive includes from the generic product.
for provider in trusty/user/app/sample/hwcrypto trusty/user/app/sample/hwcryptohal/server; do
  awk '
    { print }
    /^MODULE_LIBRARY_DEPS [:+]?= \\$/ { print "\ttrusty/user/base/lib/rng \\" }
  ' "$work/$provider/rules.mk" > "$work/$provider/rules.mk.tmp"
  mv "$work/$provider/rules.mk.tmp" "$work/$provider/rules.mk"
done

# The Rust KeyMint module calls trusty_rng_add_entropy and bindgen includes the
# RNG public header, but this pinned rule omits the corresponding direct
# library dependency. Declare it at KeyMint's module boundary so both the
# header export and implementation follow the app through the build graph.
keymint_rules="$work/trusty/user/app/keymint/rules.mk"
awk '
  { print }
  $0 == "MODULE_LIBRARY_DEPS += \\" {
    print "\ttrusty/user/base/lib/rng \\"
  }
' "$keymint_rules" > "$keymint_rules.tmp"
mv "$keymint_rules.tmp" "$keymint_rules"

qemu_atf_inc="$work/trusty/device/arm/generic-arm64/project/qemu-atf-inc.mk"
awk -v bl33="$bl33_verifier" -v root_key="$avb_root_key" -v mbedtls="$mbedtls_root" -v openssl="$openssl_dir" '
  { print }
  $0 == "ATF_MAKE_ARGS := SPD=trusty" {
    # QEMU virt exposes 14 MiB of secure DRAM at BL32_BASE (0x0e200000)
    # before the secure-device window at 0x0f000000. The generic TF-A layout
    # describes the whole gap below normal RAM, which makes Trusty allocate
    # page metadata for unbacked addresses and eventually take an external
    # abort. Pass the actual bounded arena through the Trusty X0 protocol.
    print "ATF_MAKE_ARGS += BEXOS_TRUSTY_SEC_MEM_SIZE=0x00e00000"
    print "ATF_MAKE_ARGS += TRUSTED_BOARD_BOOT=1"
    print "ATF_MAKE_ARGS += GENERATE_COT=1"
    print "ATF_MAKE_ARGS += CREATE_KEYS=1"
    print "ATF_MAKE_ARGS += ROT_KEY=" root_key
    # Pin the QEMU development BL33 signing key so the negative-test verifier
    # can have its own authenticated leaf under this exact intermediate chain.
    print "ATF_MAKE_ARGS += BL33_KEY=" root_key
    print "ATF_MAKE_ARGS += BL32=$(BL32_BIN)"
    print "ATF_MAKE_ARGS += BL33=" bl33
    print "ATF_MAKE_ARGS += MBEDTLS_DIR=" mbedtls
    if (openssl != "") {
      print "ATF_MAKE_ARGS += OPENSSL_DIR=" openssl
    }
    print "ATF_MAKE_ARGS += E=0"
  }
' "$qemu_atf_inc" > "$qemu_atf_inc.tmp"
mv "$qemu_atf_inc.tmp" "$qemu_atf_inc"
# The ordinary Trusty rule asks TF-A only for its executable images. Trusted
# Board Boot also needs the generated certificate chain; building the FIP target
# materializes that chain even though QEMU continues to load the individual
# images through semihosting so negative tests can replace BL33 independently.
"$sed_cmd" -i 's/ all sp$/ all sp fip/' "$qemu_atf_inc"

# Android's Rust prebuilt carries a built-in aarch64-unknown-trusty target.
# The host-native pinned compiler does not, so use Trusty's equivalent checked-
# in target specification for both kernel and userspace compilation.
arm64_toolchain="$work/external/lk/arch/arm64/toolchain.mk"
"$sed_cmd" -i \
  's|ARCH_arm64_RUSTFLAGS := --target=aarch64-unknown-trusty|ARCH_arm64_RUST_TARGET := $(LOCAL_DIR)/aarch64-unknown-trusty-kernel.json\
ARCH_arm64_RUSTFLAGS := --target=$(ARCH_arm64_RUST_TARGET)|' \
  "$arm64_toolchain"

platform_rules="$work/trusty/kernel/platform/generic-arm64/rules.mk"
awk '
  $0 ~ /^# vsock-rust only supports/ {
    skip = 1
    next
  }
  skip && $0 ~ /^MODULE_DEPS \+=/ {
    next
  }
  skip && $0 ~ /^[ \t]*dev\/virtio\/vsock-rust[ \t]*\\/ {
    skip = 0
    next
  }
  { print }
' "$platform_rules" > "$platform_rules.tmp"
mv "$platform_rules.tmp" "$platform_rules"

apploader_rules="$work/trusty/kernel/services/apploader/rules.mk"
awk '
  $0 ~ /^# We need to add the package tool/ {
    skip = 1
    next
  }
  skip && $0 ~ /^MODULE_DEPS \+=/ {
    next
  }
  skip && $0 ~ /^[ \t]*trusty\/user\/base\/app\/apploader\/tests\/cbor_test[ \t]*\\/ {
    skip = 0
    next
  }
  skip {
    next
  }
  { print }
' "$apploader_rules" > "$apploader_rules.tmp"
mv "$apploader_rules.tmp" "$apploader_rules"

storage_rules="$work/trusty/user/app/storage/rules.mk"
awk '
  $0 ~ /^include trusty\/user\/app\/storage\/storage_mock\/test_mock_storage_rules\.mk/ {
    next
  }
  $0 ~ /^[ \t]*\$\(LOCAL_DIR\)\/tipc_service\.c[ \t]*\\/ {
    next
  }
  $0 ~ /^MODULE_DEPS \+=/ {
    skip = 1
    next
  }
  skip && $0 ~ /^[ \t]*trusty\/user\/app\/storage\/test\/storage_host_test[ \t]*\\/ {
    skip = 0
    next
  }
  skip {
    next
  }
  { print }
' "$storage_rules" > "$storage_rules.tmp"
mv "$storage_rules.tmp" "$storage_rules"

serde_rules="$work/external/rust/android-crates-io/crates/serde/rules.mk"
awk '
  { print }
  index($0, "--cfg '\''feature=\"serde_derive\"'\''") != 0 {
    print "\t--extern serde_derive=$(TRUSTY_HOST_LIBRARY_BUILDDIR)/libserde_derive.'"$host_proc_macro_ext"' \\"
  }
' "$serde_rules" > "$serde_rules.tmp"
mv "$serde_rules.tmp" "$serde_rules"

thiserror_lib="$work/external/rust/android-crates-io/crates/thiserror/src/lib.rs"
if ! grep -q "feature(error_in_core)" "$thiserror_lib"; then
  tmp="$thiserror_lib.tmp"
  {
    echo "#![feature(error_in_core)]"
    sed -n '1,$p' "$thiserror_lib"
  } > "$tmp"
  mv "$tmp" "$thiserror_lib"
fi

# The declared Rust archive has no staged Clippy driver. Use that same pinned
# compiler for all firmware crates; repository linting remains a Bazel gate.
find "$work" -name rules.mk -type f -exec \
  "$sed_cmd" -i \
    -e 's/MODULE_RUST_USE_CLIPPY := true/MODULE_RUST_USE_CLIPPY := false/' \
    -e 's/MODULE_RUST_TESTS := true/MODULE_RUST_TESTS := false/' {} +

binder_rules="$work/frameworks/native/libs/binder/trusty/rules.mk"
awk '
  { print }
  $0 ~ /^LOCAL_DIR :=/ {
    print "MODULE_INCLUDES += external/boringssl/src/include"
    print "MODULE_COMPILEFLAGS += -Iexternal/boringssl/src/include"
  }
' "$binder_rules" > "$binder_rules.tmp"
mv "$binder_rules.tmp" "$binder_rules"

user_tasks_mk="$work/trusty/kernel/app/trusty/user-tasks.mk"
awk '
  $0 ~ /^TRUSTY_SDK_MODULES :=/ {
    print "TRUSTY_SDK_MODULES := \\"
    print "\texternal/boringssl \\"
    print "\ttrusty/kernel/lib/libc-ext \\"
    print "\ttrusty/kernel/lib/ubsan \\"
    print "\ttrusty/user/base/lib/dlmalloc \\"
    print "\ttrusty/user/base/lib/libc-trusty \\"
    print "\ttrusty/user/base/lib/syscall-stubs \\"
    print "\ttrusty/user/base/lib/tipc \\"
    print ""
    skip = 1
    next
  }
  skip && $0 ~ /^TRUSTY_SDK_MODULES \+= \$\(EXTRA_TRUSTY_SDK_MODULES\)/ {
    skip = 0
  }
  skip {
    next
  }
  { print }
' "$user_tasks_mk" > "$user_tasks_mk.tmp"
mv "$user_tasks_mk.tmp" "$user_tasks_mk"

rust_toplevel_mk="$work/external/lk/make/rust-toplevel.mk"
awk '
  $0 ~ /^# topologically sort crates/ {
    print "ifeq ($(strip $(ALLMODULE_CRATE_STEMS)),)"
    print "ALLMODULE_CRATE_STEMS_SORTED :="
    print "else"
  }
  $0 ~ /^# build "--extern/ {
    print "endif"
  }
  { print }
' "$rust_toplevel_mk" > "$rust_toplevel_mk.tmp"
mv "$rust_toplevel_mk.tmp" "$rust_toplevel_mk"

mkdir -p "$work/prebuilts/misc/linux-x86/dtc"
ln -sf "$(command -v dtc)" "$work/prebuilts/misc/linux-x86/dtc/dtc"
rm -rf "$work/prebuilts/build-tools/path/linux-x86"
rm -rf "$work/prebuilts/build-tools/linux-x86/bin"
mkdir -p "$work/prebuilts/build-tools/path/linux-x86"
ln -sf "$(command -v xxd)" "$work/prebuilts/build-tools/path/linux-x86/xxd"
ln -sf "$sed_cmd" "$work/prebuilts/build-tools/path/linux-x86/sed"
cat > "$work/prebuilts/build-tools/path/linux-x86/tac" <<EOF
#!/bin/sh
exec python3 "$reverse_lines_tool" "\$@"
EOF
chmod +x "$work/prebuilts/build-tools/path/linux-x86/tac"
mkdir -p "$work/prebuilts/build-tools/linux-x86/bin"
ln -sf "$aidl_tool" "$work/prebuilts/build-tools/linux-x86/bin/aidl"
cat > "$work/prebuilts/build-tools/linux-x86/bin/py3-cmd" <<'EOF'
#!/bin/sh
exec python3 "$@"
EOF
chmod +x "$work/prebuilts/build-tools/linux-x86/bin/py3-cmd"
mkdir -p "$work/prebuilts/clang/host/linux-x86/clang-r522817/bin"
ln -sf "$clang_bin" "$work/prebuilts/clang/host/linux-x86/clang-r522817/bin/clang"
ln -sf "$clangxx_bin" "$work/prebuilts/clang/host/linux-x86/clang-r522817/bin/clang++"
ln -sf "$ld_lld" "$work/prebuilts/clang/host/linux-x86/clang-r522817/bin/ld.lld"
ln -sf "$llvm_ar" "$work/prebuilts/clang/host/linux-x86/clang-r522817/bin/llvm-ar"
ln -sf "$llvm_cxxfilt" "$work/prebuilts/clang/host/linux-x86/clang-r522817/bin/llvm-cxxfilt"
ln -sf "$llvm_nm" "$work/prebuilts/clang/host/linux-x86/clang-r522817/bin/llvm-nm"
ln -sf "$llvm_objcopy" "$work/prebuilts/clang/host/linux-x86/clang-r522817/bin/llvm-objcopy"
ln -sf "$llvm_objdump" "$work/prebuilts/clang/host/linux-x86/clang-r522817/bin/llvm-objdump"
ln -sf "$llvm_ranlib" "$work/prebuilts/clang/host/linux-x86/clang-r522817/bin/llvm-ranlib"
ln -sf "$llvm_readelf" "$work/prebuilts/clang/host/linux-x86/clang-r522817/bin/llvm-readelf"
ln -sf "$llvm_size" "$work/prebuilts/clang/host/linux-x86/clang-r522817/bin/llvm-size"
ln -sf "$llvm_strip" "$work/prebuilts/clang/host/linux-x86/clang-r522817/bin/llvm-strip"
mkdir -p "$work/prebuilts/clang/host/linux-x86/clang-r522817/runtimes_ndk_cxx"
compiler_rt_dir="$(cd "$(dirname "$compiler_rt_marker")" && pwd)"
compiler_rt_arch="$architecture"
compiler_rt_objdir="$out_root/compiler-rt-$compiler_rt_arch"
mkdir -p "$compiler_rt_objdir"
compiler_rt_objects=
for builtin in \
  addtf3 comparetf2 divtc3 divtf3 extenddftf2 extendhftf2 extendsftf2 \
  fixtfdi fixtfsi fixtfti fixunstfdi fixunstfsi fixunstfti floatditf \
  floatsitf floattitf floatunditf floatunsitf floatuntitf multc3 multf3 \
  powitf2 subtf3 trunctfdf2 trunctfhf2 trunctfsf2; do
  source_file="$compiler_rt_dir/$builtin.c"
  object_file="$compiler_rt_objdir/$builtin.o"
  test -f "$source_file" || {
    echo "pinned compiler-rt source is missing $builtin.c" >&2
    exit 1
  }
  "$clang_bin" --target="$compiler_rt_arch-linux-gnu" -ffreestanding -fno-builtin \
    -fPIC -O2 -c "$source_file" -o "$object_file"
  compiler_rt_objects="$compiler_rt_objects $object_file"
done
if [ "$architecture" = aarch64 ]; then
fp_mode_source="$compiler_rt_dir/aarch64/fp_mode.c"
fp_mode_object="$compiler_rt_objdir/aarch64_fp_mode.o"
test -f "$fp_mode_source" || {
  echo "pinned compiler-rt source is missing aarch64/fp_mode.c" >&2
  exit 1
}
"$clang_bin" --target="$compiler_rt_arch-linux-gnu" -ffreestanding -fno-builtin \
  -fPIC -O2 -c "$fp_mode_source" -o "$fp_mode_object"
compiler_rt_objects="$compiler_rt_objects $fp_mode_object"
fi
# Word splitting is intentional: the paths above are Bazel action paths and
# cannot contain whitespace.
# shellcheck disable=SC2086
"$llvm_ar" rcs \
  "$work/prebuilts/clang/host/linux-x86/clang-r522817/runtimes_ndk_cxx/libclang_rt.builtins-$compiler_rt_arch-android.a" \
  $compiler_rt_objects
mkdir -p "$work/prebuilts/rust/host/linux-x86/1.80.1/bin"
mkdir -p "$work/prebuilts/clang-tools/linux-x86/bin"
ln -sf "$bindgen_tool" "$work/prebuilts/clang-tools/linux-x86/bin/bindgen"
mkdir -p "$work/prebuilts/rust/host/linux-x86/1.80.1/lib/rustlib/src/rust"
cp -R "$rust_src_root/." "$work/prebuilts/rust/host/linux-x86/1.80.1/lib/rustlib/src/rust/"
cat > "$work/prebuilts/rust/host/linux-x86/1.80.1/bin/rustc" <<EOF
#!/bin/sh
RUSTC_BOOTSTRAP=1 exec "$rustc_tool" "\$@"
EOF
cat > "$work/prebuilts/rust/host/linux-x86/1.80.1/bin/rustfmt" <<EOF
#!/bin/sh
exec "$rustfmt_tool" "\$@"
EOF
cat > "$work/prebuilts/rust/host/linux-x86/1.80.1/bin/rustdoc" <<EOF
#!/bin/sh
RUSTC_BOOTSTRAP=1 exec "$rustdoc_tool" "\$@"
EOF
chmod +x "$work/prebuilts/rust/host/linux-x86/1.80.1/bin/rustc"
chmod +x "$work/prebuilts/rust/host/linux-x86/1.80.1/bin/rustfmt"
chmod +x "$work/prebuilts/rust/host/linux-x86/1.80.1/bin/rustdoc"

if [ "$architecture" = x86_64 ]; then
  if [[ -n "${BEXOS_TRUSTY_X86_GENERATION_CONFIG:-}" ]]; then
    python3 "$x86_config" "$work" "$project_mk" "$BEXOS_TRUSTY_X86_GENERATION_CONFIG"
  else
    python3 "$x86_config" "$work" "$project_mk"
  fi
fi

# The pinned Trusty make graph does not express the dependency from every SDK
# object to the staged libc/libc++ headers in ALL_SDK_INCLUDES.  A parallel
# build can consequently compile libc-ext or BoringSSL before stdio.h or
# <memory> has been installed in the SDK sysroot.  Keep this upstream firmware
# build serial and let Bazel parallelize independent top-level actions.
PATH="$work/prebuilts/build-tools/path/linux-x86:$PATH" "$make_cmd" -C "$work" \
  PROJECT=bexos-trusty-qemu-debug \
  PACKAGE_TRUSTY_IMAGES_ONLY=true \
  LKROOT=external/lk \
  ARCH_x86_64_TOOLCHAIN_PREFIX="$work/prebuilts/clang/host/linux-x86/clang-r522817/bin/llvm-" \
  ARCH_arm64_TOOLCHAIN_PREFIX="$work/prebuilts/clang/host/linux-x86/clang-r522817/bin/llvm-" \
  CLANG_BINDIR="$work/prebuilts/clang/host/linux-x86/clang-r522817/bin" \
  CLANG_TOOLS_BINDIR="$work/prebuilts/clang-tools/linux-x86/bin" \
  BINDGEN_CLANG_PATH="$clang_bin" \
  BINDGEN_LIBCLANG_PATH="$libclang_dir" \
  BUILDTOOLS_BINDIR="$work/prebuilts/build-tools/linux-x86/bin" \
  PATH_TOOLS_BINDIR="$work/prebuilts/build-tools/path/linux-x86" \
  TRUSTY_TOP="$work" \
  PROTOC_TOOL="$protoc_tool" \
  RUST_BINDIR="$work/prebuilts/rust/host/linux-x86/1.80.1/bin" \
  RUST_HOST_LIBDIR="$rust_host_libdir" \
  PY3=python3 \
  BUILDROOT="$out_root/build-root"

build_dir="$out_root/build-root/build-bexos-trusty-qemu-debug"
images="bl1.bin bl2.bin bl31.bin lk.bin"
if [ "$architecture" = x86_64 ]; then images="lk.bin"; fi
for image in $images; do
  source_file="$(find "$build_dir" -name "$image" -type f | head -n 1)"
  test -n "$source_file" || { echo "Trusty build did not produce $image" >&2; exit 1; }
  cp "$source_file" "$out_root/$image"
done

if [ "$architecture" = aarch64 ]; then cp "$bl33_verifier" "$out_root/bl33.bin"; fi

lk_elf="$(find "$build_dir" -name lk.elf -type f | head -n 1)"
test -n "$lk_elf" || { echo "Trusty build did not produce lk.elf" >&2; exit 1; }
cp "$lk_elf" "$out_root/lk.elf"

if [ "$architecture" = aarch64 ]; then
for certificate in \
  tb_fw.crt \
  trusted_key.crt \
  soc_fw_key.crt \
  tos_fw_key.crt \
  nt_fw_key.crt \
  soc_fw_content.crt \
  tos_fw_content.crt \
  nt_fw_content.crt; do
  source_file="$(find "$build_dir" -name "$certificate" -type f | head -n 1)"
  test -n "$source_file" || {
    echo "authenticated TF-A build did not produce $certificate" >&2
    exit 1
  }
  cp "$source_file" "$out_root/$certificate"
done
cp "$rollback_verifier" "$out_root/bl33-rollback.bin"
"$work/external/arm-trusted-firmware/tools/cert_create/cert_create" \
  --new-keys --nt-fw-key "$avb_root_key" --ntfw-nvctr 0 \
  --nt-fw "$rollback_verifier" \
  --nt-fw-cert "$out_root/rollback_nt_fw_content.crt"
fi

cp "$rpmb_tool" "$out_root/rpmb_dev"

chmod +x "$out_root/rpmb_dev"

rpmb_data="$(find "$build_dir" -name RPMB_DATA -type f | head -n 1)"
if test -z "$rpmb_data"; then
  "$out_root/rpmb_dev" --dev "$out_root/RPMB_DATA" --init --size 2048
else
  cp "$rpmb_data" "$out_root/RPMB_DATA"
fi
