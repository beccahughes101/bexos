use bexos_secure_firmware::{
    Component,
    selection::{Identity, Slot},
    store::{self, BlockDevice, Error, SECTOR_BYTES},
};
use std::collections::BTreeMap;
const BUNDLE: &[u8] = include_bytes!(env!("TEST_FIRMWARE"));
const ROOT: &[u8] = include_bytes!(env!("TEST_ROOT"));

#[test]
fn reused_upload_allocation_still_requires_complete_authenticated_readback() {
    let mut good = Device::default();
    let mut bytes = BUNDLE.to_vec();
    let identity = store::install_in_place(
        &mut good,
        Component::Hypervisor,
        Identity::INITIAL,
        &mut bytes,
        ROOT,
        1,
    )
    .unwrap();
    assert_eq!(bytes, BUNDLE);
    assert_eq!(identity.generation, 2);
    for fail in 1..=good.operation {
        let mut disk = Device {
            fail: Some(fail),
            ..Device::default()
        };
        let mut bytes = BUNDLE.to_vec();
        assert!(
            store::install_in_place(
                &mut disk,
                Component::Hypervisor,
                Identity::INITIAL,
                &mut bytes,
                ROOT,
                1
            )
            .is_err()
        );
    }
    let mut disk = Device {
        corrupt_readback: true,
        ..Device::default()
    };
    assert_eq!(
        store::install_in_place(
            &mut disk,
            Component::Hypervisor,
            Identity::INITIAL,
            &mut BUNDLE.to_vec(),
            ROOT,
            1
        ),
        Err(Error::Image)
    );
}
#[derive(Clone, Default)]
struct Device {
    architecture: Option<bexos_secure_firmware::Architecture>,
    durable: BTreeMap<u64, [u8; SECTOR_BYTES]>,
    pending: BTreeMap<u64, [u8; SECTOR_BYTES]>,
    operation: usize,
    fail: Option<usize>,
    corrupt_readback: bool,
    lost_flush_reply: bool,
}
impl Device {
    fn operation(&mut self) -> Result<(), Error> {
        self.operation += 1;
        if self.fail == Some(self.operation) {
            Err(Error::Device)
        } else {
            Ok(())
        }
    }
}
impl BlockDevice for Device {
    fn architecture(&self) -> bexos_secure_firmware::Architecture {
        self.architecture
            .unwrap_or(bexos_secure_firmware::Architecture::X86_64)
    }
    fn sectors(&self) -> u64 {
        store::DISK_BYTES / SECTOR_BYTES as u64
    }
    fn read(&mut self, sector: u64, output: &mut [u8; SECTOR_BYTES]) -> Result<(), Error> {
        self.operation()?;
        *output = *self
            .pending
            .get(&sector)
            .or(self.durable.get(&sector))
            .unwrap_or(&[0; SECTOR_BYTES]);
        if self.corrupt_readback && sector == store::slot_sector(Component::Hypervisor, Slot::B) + 1
        {
            output[0] ^= 1;
        }
        Ok(())
    }
    fn write(&mut self, sector: u64, bytes: &[u8; SECTOR_BYTES]) -> Result<(), Error> {
        self.operation()?;
        assert!(sector < self.sectors());
        self.pending.insert(sector, *bytes);
        Ok(())
    }
    fn flush(&mut self) -> Result<(), Error> {
        if self.lost_flush_reply && self.fail == Some(self.operation + 1) {
            self.durable.append(&mut self.pending);
        }
        self.operation()?;
        self.durable.append(&mut self.pending);
        Ok(())
    }
}
#[test]
fn every_interrupted_install_preserves_active_slot_and_never_reports_success() {
    let active_sector = store::slot_sector(Component::Hypervisor, Slot::A);
    let mut initial = Device::default();
    initial.durable.insert(active_sector, [0xa5; SECTOR_BYTES]);
    let mut successful = initial.clone();
    let mut scratch = vec![0; BUNDLE.len()];
    let identity = store::install(
        &mut successful,
        Component::Hypervisor,
        Identity::INITIAL,
        BUNDLE,
        ROOT,
        1,
        &mut scratch,
    )
    .unwrap();
    assert_eq!(identity.slot, Slot::B);
    assert_eq!(identity.generation, 2);
    let operations = successful.operation;
    for fail in 1..=operations {
        for lost_flush_reply in [false, true] {
            let mut disk = initial.clone();
            disk.fail = Some(fail);
            disk.lost_flush_reply = lost_flush_reply;
            assert!(
                store::install(
                    &mut disk,
                    Component::Hypervisor,
                    Identity::INITIAL,
                    BUNDLE,
                    ROOT,
                    1,
                    &mut scratch
                )
                .is_err(),
                "failed I/O {fail} was acknowledged"
            );
            // A crash discards writes not covered by a successful flush.
            disk.pending.clear();
            disk.fail = None;
            assert_eq!(disk.durable[&active_sector], [0xa5; SECTOR_BYTES]);
            // An unpublished inactive image may be complete, but can only be
            // loaded with an independently authenticated journal identity.
            if let Ok(image) = store::load(
                &mut disk,
                Component::Hypervisor,
                identity,
                ROOT,
                1,
                &mut scratch,
            ) {
                assert_eq!(image.digest, identity.digest);
            }
        }
    }
    successful.pending.clear();
    assert_eq!(
        store::load(
            &mut successful,
            Component::Hypervisor,
            identity,
            ROOT,
            2,
            &mut scratch
        )
        .unwrap()
        .digest,
        identity.digest
    );
    assert!(
        store::load(
            &mut successful,
            Component::Hypervisor,
            Identity {
                digest: [1; 32],
                ..identity
            },
            ROOT,
            1,
            &mut scratch
        )
        .is_err()
    );
    assert!(
        store::load(
            &mut successful,
            Component::Trusty,
            identity,
            ROOT,
            1,
            &mut scratch
        )
        .is_err()
    );
    assert!(
        store::load(
            &mut successful,
            Component::Hypervisor,
            identity,
            ROOT,
            3,
            &mut scratch
        )
        .is_err()
    );
}
#[test]
fn readback_corruption_and_bad_signatures_cannot_publish_an_installation() {
    let mut disk = Device {
        corrupt_readback: true,
        ..Device::default()
    };
    let mut scratch = vec![0; BUNDLE.len()];
    assert!(
        store::install(
            &mut disk,
            Component::Hypervisor,
            Identity::INITIAL,
            BUNDLE,
            ROOT,
            1,
            &mut scratch
        )
        .is_err()
    );
    let mut disk = Device::default();
    let mut changed = BUNDLE.to_vec();
    *changed.last_mut().unwrap() ^= 1;
    assert!(
        store::install(
            &mut disk,
            Component::Hypervisor,
            Identity::INITIAL,
            &changed,
            ROOT,
            1,
            &mut scratch
        )
        .is_err()
    );
    assert_eq!(
        disk.operation, 0,
        "unauthenticated candidate touched firmware disk"
    );
}

#[test]
fn arm_slots_authenticate_arm_images_and_reject_cross_architecture_before_writes() {
    use bexos_secure_firmware::Architecture;
    let arm_bundle = include_bytes!(env!("ARM_TEST_FIRMWARE"));
    let mut disk = Device {
        architecture: Some(Architecture::Aarch64),
        ..Device::default()
    };
    let mut scratch = vec![0; arm_bundle.len()];
    let image = store::install(
        &mut disk,
        Component::Trusty,
        Identity::INITIAL,
        arm_bundle,
        ROOT,
        1,
        &mut scratch,
    )
    .unwrap();
    assert_eq!(image.generation, 2);
    assert_eq!(
        store::load(&mut disk, Component::Trusty, image, ROOT, 1, &mut scratch)
            .unwrap()
            .generation,
        2
    );
    let before = disk.durable.clone();
    assert_eq!(
        store::install(
            &mut disk,
            Component::Hypervisor,
            Identity::INITIAL,
            BUNDLE,
            ROOT,
            1,
            &mut scratch
        ),
        Err(Error::Image)
    );
    assert_eq!(disk.durable, before);
    let mut x86 = Device::default();
    assert_eq!(
        store::install(
            &mut x86,
            Component::Trusty,
            Identity::INITIAL,
            arm_bundle,
            ROOT,
            1,
            &mut scratch
        ),
        Err(Error::Image)
    );
    assert_eq!(x86.operation, 0);
    disk.architecture = Some(Architecture::X86_64);
    assert!(store::load(&mut disk, Component::Trusty, image, ROOT, 1, &mut scratch).is_err());
}
