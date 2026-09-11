//! Root-owned image authentication followed by a real Trusty RPMB decision.
use bexos_secure_monitor::boot_verify::{self, Verified};
use bexos_trusty_boot::{approval, avb::Avb, ql::Transport};
#[cfg(not(feature = "external_payload"))]
static KERNEL: &[u8] = include_bytes!(env!("VERIFIED_KERNEL"));
#[cfg(not(feature = "external_payload"))]
static BOOTFS: &[u8] = include_bytes!(env!("VERIFIED_BOOTFS"));
#[cfg(not(feature = "external_payload"))]
const METADATA: &[u8] = include_bytes!(env!("VERIFIED_METADATA"));
const ROOT: &[u8] = include_bytes!(env!("VERIFIED_ROOT"));

#[cfg(feature = "external_payload")]
pub use crate::payload::{bootfs, kernel, metadata};
#[cfg(not(feature = "external_payload"))]
pub fn kernel() -> &'static [u8] {
    KERNEL
}
#[cfg(not(feature = "external_payload"))]
pub fn bootfs() -> &'static [u8] {
    BOOTFS
}
#[cfg(not(feature = "external_payload"))]
fn metadata() -> &'static [u8] {
    METADATA
}

pub fn verify() -> Verified {
    #[cfg(feature = "external_payload")]
    crate::payload::load();
    match boot_verify::verify(metadata(), ROOT, kernel(), bootfs()) {
        Ok(verified) => verified,
        Err(_) => {
            crate::log("monitor-runtime: boot payload authentication rejected\n");
            crate::halt();
        }
    }
}
pub fn approve(transport: impl Transport, verified: Verified) -> approval::Approval {
    #[cfg(feature = "rollback_fixture")]
    let transport = {
        let mut transport = transport;
        crate::rollback_fixture::provision_journal(&mut transport);
        transport
    };
    #[cfg(feature = "secure_product")]
    let (transport, committed) = {
        let mut transport = transport;
        let records = crate::firmware_generations::committed(&mut transport).unwrap_or_else(|error| {
            crate::log(if error == bexos_trusty_boot::journal::Error::Rollback {
                "monitor-runtime: stale generation rejected by Trusty committed replacement floor\n"
            } else {
                "monitor-runtime: authenticated replacement journal unavailable\nmonitor-runtime: Trusty rollback state unavailable; refusing execution\n"
            });
            crate::halt();
        });
        (transport, records)
    };
    let mut avb = match Avb::connect(transport) {
        Ok(avb) => avb,
        Err(_) => {
            crate::log("monitor-runtime: Trusty rollback state unavailable; refusing execution\n");
            crate::halt();
        }
    };
    #[cfg(feature = "rollback_fixture")]
    crate::rollback_fixture::provision(&mut avb, verified);
    #[cfg(feature = "secure_product")]
    let result = crate::firmware_generations::approve_chain(
        &mut avb,
        approval::Generation {
            generation: verified.generation,
            location: verified.rollback_location,
        },
        committed,
    )
    .map(|approved| {
        crate::firmware_generations::install(approved[1], approved[2]);
        approved[0]
    });
    #[cfg(not(feature = "secure_product"))]
    let result = approval::approve(&mut avb, verified.generation, verified.rollback_location);
    let approval = match result {
        Ok(approval) => approval,
        Err(error) => {
            crate::log(match error {
                approval::Error::Rollback => {
                    "monitor-runtime: stale generation rejected by Trusty RPMB floor\n"
                }
                approval::Error::Unlocked => {
                    "monitor-runtime: unlocked Trusty boot state rejected\n"
                }
                approval::Error::InvalidGeneration => {
                    "monitor-runtime: invalid boot generation rejected\n"
                }
                approval::Error::Transport(_) => {
                    "monitor-runtime: Trusty rollback state unavailable; refusing execution\n"
                }
            });
            crate::halt();
        }
    };
    // A successful seal must revoke even the boot owner's write authority.
    // This writes the same floor, so an incorrect acceptance cannot raise it.
    assert_eq!(
        avb.write_rollback(approval.location(), approval.floor()),
        Err(bexos_trusty_boot::ql::Error::Rejected)
    );
    avb.close().expect("release approved boot QL device");
    crate::log("monitor-runtime: approved boot generation=\n");
    crate::hex(approval.generation());
    crate::log("monitor-runtime: authenticated payload and Trusty rollback approval verified\n");
    approval
}
