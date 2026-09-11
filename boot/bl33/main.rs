#![no_main]
#![no_std]

extern crate alloc;

mod heap;
mod rpmb;
mod trusty;

use bexos_avb::{verify_partition, verify_vbmeta};
use bexos_boot::{
    BOOT_EVIDENCE_ADDR, BOOT_EVIDENCE_FLAG_RPMB_ANTI_ROLLBACK, BOOT_EVIDENCE_FLAG_SECURE_BOOT,
    BOOTFS_ADDR, BootEvidenceV1, BootHandoff, HANDOFF_ADDR, KERNEL_START, MAX_VBMETA_LEN,
    VBMETA_ADDR,
};
use bexos_kernel_core::bootfs::Bootfs;
use core::arch::asm;
use core::fmt::{self, Write};
use core::panic::PanicInfo;
use sha2::{Digest, Sha256};

core::arch::global_asm!(
    r#"
    .section .text._start, "ax"
    .balign 16
    .global _start
    .type _start, %function
_start:
    adrp x9, __stack_top
    add x9, x9, :lo12:__stack_top
    mov sp, x9
    b bl33_main
    .size _start, . - _start
"#
);

// Explicit QEMU development root. Production boards replace this constant with
// provisioned immutable key material through their BL33 build contract.
const DEV_MODULUS_HEX: &str = concat!(
    "9b70a2b72e080e26ca85728e591f46a3b6d58b83608e8d5cf3f76997a9ad8970",
    "de4f0779363d369f5b06b4c625191031a2077f4194d575d7f7fef0f615f7245e",
    "b6dbf6f048e8d7cbe654d605979fa13281dd86a36b0915ea192610657526e5e6",
    "31f1f30811d9edb709f57be703558cdf6cba7fbce164179612e1197a89f9be6d",
    "4be12de7a422f9c00abe66f3eba569aa51068e3b17dc8e4715791567cfc31dc5",
    "7f79eba37b8247af7735c81e8e8a261e3efd9de52d648c20d665a4a110577c1a",
    "3830238f8be5ef4813d3fdf424c525e7545ad8812d11a749640dfdf33d04f50c",
    "9c844cb8840774000d115792d07cf1f6f08bb156df484f0a754f19afdf89eb87"
);

#[global_allocator]
static ALLOCATOR: heap::Bump = heap::Bump;

#[unsafe(no_mangle)]
pub extern "C" fn bl33_main(dtb: u64) -> ! {
    zero_bss();
    log("bl33: authenticated verifier entered\n");
    match verify_and_boot(dtb) {
        Ok(()) => fail("returned from kernel"),
        Err(error) => fail(error),
    }
}

fn verify_and_boot(dtb: u64) -> Result<(), &'static str> {
    let handoff = unsafe { core::ptr::read(HANDOFF_ADDR as *const BootHandoff) };
    if !handoff.validate(KERNEL_START, KERNEL_START + 4096) {
        return Err("invalid handoff");
    }
    let vbmeta = vbmeta_bytes()?;
    let mut modulus = [0u8; 256];
    decode_hex(DEV_MODULUS_HEX.as_bytes(), &mut modulus)?;
    let verified = verify_vbmeta(vbmeta, &modulus).map_err(|_| "vbmeta authentication failed")?;
    if verified.flags != 0 || verified.rollback_index_location > 31 {
        return Err("unsupported vbmeta policy");
    }
    let kernel_desc = verified
        .hash_descriptor(b"kernel")
        .map_err(|_| "missing kernel descriptor")?;
    let kernel_len = usize::try_from(kernel_desc.image_size).map_err(|_| "kernel too large")?;
    if kernel_len == 0 {
        return Err("empty kernel descriptor");
    }
    let kernel_end = KERNEL_START
        .checked_add(kernel_desc.image_size)
        .ok_or("kernel address overflow")?;
    if !handoff.validate(KERNEL_START, kernel_end) {
        return Err("kernel outside verified handoff layout");
    }
    let kernel = unsafe { core::slice::from_raw_parts(KERNEL_START as *const u8, kernel_len) };
    verify_partition(kernel_desc, kernel).map_err(|_| "kernel digest failed")?;

    let bootfs = unsafe {
        core::slice::from_raw_parts(BOOTFS_ADDR as *const u8, handoff.bootfs_len as usize)
    };
    let bootfs_desc = verified
        .hash_descriptor(b"bootfs")
        .map_err(|_| "missing BootFS descriptor")?;
    verify_partition(bootfs_desc, bootfs).map_err(|_| "BootFS digest failed")?;
    let parsed = Bootfs::parse(bootfs).map_err(|_| "malformed BootFS")?;
    let policy = parsed
        .find("/boot/platform.pcfg")
        .map_err(|_| "malformed policy entry")?
        .ok_or("missing policy")?;
    let policy_desc = verified
        .hash_descriptor(b"platform-policy")
        .map_err(|_| "missing policy descriptor")?;
    verify_partition(policy_desc, policy.bytes).map_err(|_| "policy digest failed")?;

    log("bl33: vbmeta and boot partitions verified\n");
    trusty::approve(verified.rollback_index, verified.rollback_index_location)?;
    let evidence = BootEvidenceV1::verified_bl33(
        verified.rollback_index,
        BOOT_EVIDENCE_FLAG_SECURE_BOOT | BOOT_EVIDENCE_FLAG_RPMB_ANTI_ROLLBACK,
        Sha256::digest(kernel).into(),
        Sha256::digest(bootfs).into(),
        Sha256::digest(policy.bytes).into(),
        Sha256::digest(b"bexos.ta.orchestrator").into(),
        Sha256::digest(modulus).into(),
    );
    unsafe { core::ptr::write(BOOT_EVIDENCE_ADDR as *mut BootEvidenceV1, evidence) };
    log("bl33: verified boot approved before kernel entry\n");
    let kernel_entry: extern "C" fn(u64) -> ! =
        unsafe { core::mem::transmute(KERNEL_START as usize) };
    if dtb >= bexos_boot::RAM_START
        && dtb
            .checked_add(40)
            .is_some_and(|end| end <= bexos_boot::RAM_END)
    {
        let size =
            unsafe { u32::from_be(core::ptr::read_unaligned((dtb + 4) as *const u32)) } as u64;
        if (40..=2 * 1024 * 1024).contains(&size)
            && dtb
                .checked_add(size)
                .is_some_and(|end| end <= bexos_boot::RAM_END)
        {
            let tree = unsafe { core::slice::from_raw_parts(dtb as *const u8, size as usize) };
            if let Some(framebuffer) = bexos_boot::framebuffer::simple_framebuffer(tree) {
                let h = unsafe { &mut *(HANDOFF_ADDR as *mut BootHandoff) };
                h.normalize_legacy_extensions();
                h.version = bexos_boot::BOOT_HANDOFF_VERSION;
                h.framebuffer = framebuffer;
            }
        }
    }
    kernel_entry(HANDOFF_ADDR)
}

fn vbmeta_bytes() -> Result<&'static [u8], &'static str> {
    let header = unsafe { core::slice::from_raw_parts(VBMETA_ADDR as *const u8, 256) };
    if &header[..4] != b"AVB0" {
        return Err("invalid vbmeta header");
    }
    let auth = u64::from_be_bytes(header[12..20].try_into().unwrap());
    let aux = u64::from_be_bytes(header[20..28].try_into().unwrap());
    let len = 256u64
        .checked_add(auth)
        .and_then(|v| v.checked_add(aux))
        .ok_or("vbmeta overflow")?;
    if len > MAX_VBMETA_LEN {
        return Err("vbmeta too large");
    }
    Ok(unsafe { core::slice::from_raw_parts(VBMETA_ADDR as *const u8, len as usize) })
}

fn decode_hex(input: &[u8], output: &mut [u8]) -> Result<(), &'static str> {
    if input.len() != output.len() * 2 {
        return Err("invalid root key");
    }
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = (nibble(input[index * 2])? << 4) | nibble(input[index * 2 + 1])?;
    }
    Ok(())
}
fn nibble(value: u8) -> Result<u8, &'static str> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err("invalid root key"),
    }
}

fn zero_bss() {
    unsafe extern "C" {
        static mut __bss_start: u8;
        static mut __bss_end: u8;
    }
    unsafe {
        core::ptr::write_bytes(
            core::ptr::addr_of_mut!(__bss_start),
            0,
            core::ptr::addr_of!(__bss_end) as usize - core::ptr::addr_of!(__bss_start) as usize,
        );
    }
}
fn log(message: &str) {
    let mut uart = Uart;
    let _ = uart.write_str(message);
}
fn fail(message: &'static str) -> ! {
    log("bl33: FATAL: ");
    log(message);
    log("\n");
    loop {
        unsafe {
            asm!("wfi", options(nomem, nostack));
        }
    }
}
struct Uart;
impl Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            unsafe {
                core::ptr::write_volatile(0x0900_0000 as *mut u8, byte);
            }
        }
        Ok(())
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    fail("panic")
}
