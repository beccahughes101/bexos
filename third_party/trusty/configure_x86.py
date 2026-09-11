#!/usr/bin/env python3
"""Apply x86-only build adaptation to the action-local pinned Trusty tree."""
from pathlib import Path
import json
import sys

work, project = map(Path, sys.argv[1:3])
generation = 1
if len(sys.argv) == 4:
    import re
    config = Path(sys.argv[3]).read_text()
    match = re.fullmatch(r'\s*architecture:\s*"x86_64"\s+generation:\s*([1-9][0-9]*)\s*', config)
    if not match or int(match[1]) >= 2**63:
        raise SystemExit("invalid x86 firmware generation configuration")
    generation = int(match[1])
text = project.read_text()
text = text.replace("SMP_MAX_CPUS := 4", "SMP_MAX_CPUS := 1")
text = text.replace("include project/generic-arm64-debug.mk", "include project/generic-x86_64-inc.mk")
text = text[:text.index("ATF_DEBUG :=")]
text += "\n# Standalone Q35 development instance; no ARM secure monitor.\nWITH_TRUSTY_VIRTIO_IPC_DEV := false\n"
text += "GLOBAL_KERNEL_COMPILEFLAGS += -ffreestanding\n"
text += f"GLOBAL_DEFINES += BEXOS_TRUSTY_GENERATION={generation}\n"
text += "MODULES += trusty/device/x86/bexos/rpmb\n"
text += "MODULES += trusty/device/x86/bexos/monitor\n"
project.write_text(text)
toolchain = work / "external/lk/arch/x86/toolchain.mk"
text = toolchain.read_text().replace("ARCH_x86_RUSTFLAGS := --target=x86_64-unknown-trusty", "ARCH_x86_RUST_TARGET := $(LOCAL_DIR)/x86_64-unknown-trusty-user.json\nARCH_x86_RUSTFLAGS := --target=$(ARCH_x86_RUST_TARGET)")
toolchain.write_text(text)
target = work / "external/lk/arch/x86/x86_64-unknown-trusty-kernel.json"
spec = json.loads(target.read_text())
spec["features"] = "+sse2"
spec.pop("soft-float", None)
(target.parent / "x86_64-unknown-trusty-user.json").write_text(json.dumps(spec))

serde = work / "external/rust/android-crates-io/crates/serde/src/lib.rs"
serde.write_text("#![feature(error_in_core)]\n" + serde.read_text())
# Rust 1.80 lacks Box::as_ptr; preserve the upstream no-reborrow semantics by
# backporting the methods through Box's existing Unique pointer field.
boxed = work / "prebuilts/rust/host/linux-x86/1.80.1/lib/rustlib/src/rust/library/alloc/src/boxed.rs"
boxed.write_text(boxed.read_text() + '''
impl<T: ?Sized, A: Allocator> Box<T, A> {
    #[inline]
    #[stable(feature = "bexos_box_as_ptr", since = "1.80.1")]
    /// Return the allocation pointer without creating a reference to its contents.
    pub fn as_ptr(this: &Self) -> *const T { this.0.as_ptr() }
    #[inline]
    #[stable(feature = "bexos_box_as_ptr", since = "1.80.1")]
    /// Return the mutable allocation pointer without reborrowing its contents.
    pub fn as_mut_ptr(this: &mut Self) -> *mut T { this.0.as_ptr() }
}
''')
support = work / "external/lk/lib/rust_support/lib.rs"
support.write_text(support.read_text().replace("#![feature(box_as_ptr)]", ""))
# This arithmetic is over the nonzero hypervisor page size.
import re
for path in (work / "external/lk/lib/libhypervisor/src").glob("*.rs"):
    text = path.read_text().replace("#![feature(unsigned_is_multiple_of)]", "")
    text = re.sub(r"(\w+)\.is_multiple_of\((\w+)\)", r"(\1 % \2 == 0)", text)
    path.write_text(text)
# Acceptance-only endpoints and clients never enter the standard bundle.
if (work / "trusty/user/app/bexos/authmgr_acceptance").exists():
    app = work / "trusty/device/x86/bexos/acceptance"
    (app / "boot_ready.rs").write_text((work / "trusty/user/app/bexos/authmgr_acceptance/boot_ready.rs").read_text())
    fields = dict(re.findall(r'^(\w+):\s*(.+)$', (app / "manifest.prototxt").read_text(), re.MULTILINE))
    (app / "manifest.json").write_text(json.dumps({k: json.loads(v) for k,v in fields.items()}))
    project.write_text(project.read_text() + "\nTRUSTY_BUILTIN_USER_TASKS += trusty/device/x86/bexos/acceptance\nTRUSTY_ALL_USER_TASKS := $(TRUSTY_BUILTIN_USER_TASKS)\n")
    for relative in ["trusty/user/app/gatekeeper/ipc/gatekeeper_ipc.cpp", "trusty/user/app/avb/ipc/avb_ipc.cpp", "trusty/user/app/bexos/orchestrator/main.c"]:
        path = work / relative
        path.write_text(path.read_text().replace("IPC_PORT_ALLOW_NS_CONNECT", "(IPC_PORT_ALLOW_NS_CONNECT | IPC_PORT_ALLOW_TA_CONNECT)"))

virtio = work / "external/rust/android-crates-io/crates/virtio-drivers-and-devices/src/lib.rs"
virtio.write_text("#![feature(raw_ref_op)]\n" + virtio.read_text())

# The x86 kernel has Rust modules; use portable printf in its make include.
rust_make = work / "external/lk/make/rust-toplevel.mk"
rust_make.write_text(rust_make.read_text().replace('echo -n "ALLMODULE_CRATE_STEMS_SORTED := "', 'printf "%s" "ALLMODULE_CRATE_STEMS_SORTED := "'))

# Q35 TCG is an explicitly unprotected development backend. Match its actual
# CPUID signature, retain rejection of unknown hypervisors, and expose none of
# the protected KVM memory-sharing, MMIO-guard, or device-assignment operations.
backend_dir = work / "packages/modules/Virtualization/libs/libhypervisor_backends/src"
backend = backend_dir / "hypervisor.rs"
text = backend.read_text()
text = text.replace("enum HypervisorBackend {", '''#[cfg(target_arch = "x86_64")]
struct QemuTcgDevelopmentHypervisor;
#[cfg(target_arch = "x86_64")]
impl Hypervisor for QemuTcgDevelopmentHypervisor {}

enum HypervisorBackend {
    #[cfg(target_arch = "x86_64")]
    QemuTcgDevelopment,''')
text = text.replace("match self {\n            Self::RegularKvm", '''match self {
            #[cfg(target_arch = "x86_64")]
            Self::QemuTcgDevelopment => &QemuTcgDevelopmentHypervisor,
            Self::RegularKvm''')
backend.write_text(text)
kvm = backend_dir / "hypervisor/kvm_x86.rs"
text = kvm.read_text().replace("    match cpuid {", '''    const QEMU_TCG_CPUID: u128 = u128::from_le_bytes(*b"TCGTCGTCGTCG\\0\\0\\0\\0");
    match cpuid {
        QEMU_TCG_CPUID => Ok(HypervisorBackend::QemuTcgDevelopment),''')
kvm.write_text(text)
