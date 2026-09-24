#![no_std]
#![no_main]
extern crate alloc;
use alloc::format;
use alloc::vec::Vec;
use bexos_d1_pci::*;
use bexos_userspace::live_migration::State;
use bexos_userspace::service_control::ControlState;
use bexos_userspace::{Channel, Memory, Startup, log};
mod runtime;
bexos_userspace::entry!(run);
fn run(channel: u64) -> ! {
    let channel = Channel(channel);
    let startup = Startup::receive(channel).unwrap();
    if startup.migration_target {
        match bexos_userspace::live_migration::receive::<runtime::state::Runtime>(
            channel,
            startup.migration_generation,
        ) {
            Ok(state) => runtime::serve(state),
            Err(_) => bexos_userspace::exit(),
        }
    }
    let registry = startup
        .resources
        .first()
        .copied()
        .map(Channel)
        .unwrap_or_else(|| {
            log("pci: registry channel missing from startup resources\n");
            bexos_userspace::exit();
        });
    let h = match Memory::physical(bexos_userspace::syscall::pci_ecam_base(), ECAM_SIZE) {
        Ok(h) => h,
        Err(e) => {
            log(&format!("pci: ECAM grant failed {e:?}\n"));
            bexos_userspace::exit();
        }
    };
    let va = match Memory::map(h, ECAM_SIZE, 6) {
        Ok(va) => va,
        Err(e) => {
            log(&format!("pci: ECAM map failed {e:?}\n"));
            bexos_userspace::exit();
        }
    };
    log("pci: EL0 enumeration started\n");
    let (mmio_base, mmio_limit) = bexos_userspace::syscall::pci_mmio_window();
    let root = RootBusConfig::new(mmio_base, mmio_limit).unwrap_or_else(|_| {
        log("pci: invalid platform MMIO window\n");
        bexos_userspace::exit();
    });
    let (bus_start, bus_end) = bexos_userspace::syscall::pci_bus_range();
    log(&format!(
        "pci: root segment={} buses={}-{} ecam={:#x} mmio={:#x}-{:#x}\n",
        bexos_userspace::syscall::pci_segment(),
        bus_start,
        bus_end,
        bexos_userspace::syscall::pci_ecam_base(),
        mmio_base,
        mmio_limit
    ));
    let mut bus = PciRootBus::new(unsafe { EcamConfigSpace::new(va as usize) }, root);
    let mut nodes = match bus.enumerate_bus0() {
        Ok(nodes) => nodes,
        Err(e) => {
            log(&format!("pci: enumeration failed {e:?}\n"));
            bexos_userspace::exit();
        }
    };
    let mut ports = bus
        .configure_root_ports()
        .expect("PCI root-port configuration");
    for port in &mut ports {
        for device in bus
            .discover_bus(port.secondary)
            .expect("PCI downstream discovery")
        {
            nodes.push(
                bus.initialize_downstream(port, device)
                    .expect("PCI downstream BARs"),
            );
        }
    }
    let mut known = Vec::new();
    let mut device_controls = Vec::new();
    for node in nodes {
        let property = |key| {
            node.properties
                .iter()
                .find(|p| p.key == key)
                .map(|p| p.value)
                .unwrap_or(0)
        };
        log(&format!(
            "pci: device node={} vendor={:04x} device={:04x} class={:02x} subclass={:02x} prog_if={:02x} bars={}\n",
            node.node_id,
            property("pci.vendor_id"),
            property("pci.device_id"),
            property("pci.class"),
            property("pci.subclass"),
            property("pci.prog_if"),
            node.bars.len()
        ));
        known.push(node.node_id);
        let registration = runtime::registry::register(registry, &node).unwrap_or_else(|_| {
            log(&format!(
                "pci: registry registration send failed node={}\n",
                node.node_id
            ));
            bexos_userspace::exit();
        });
        registry
            .recv()
            .map_err(|_| ())
            .and_then(runtime::registry::response)
            .unwrap_or_else(|_| {
                log(&format!(
                    "pci: registry registration failed node={}\n",
                    node.node_id
                ));
                bexos_userspace::exit();
            });
        device_controls.push(runtime::state::DeviceControl {
            node: node.node_id,
            channel: registration.control,
            interrupt: registration.interrupt,
        });
    }
    log("pci: EL0 enumeration and device registration complete\n");
    Startup::ready(channel).unwrap();
    let state = runtime::state::Runtime {
        control: ControlState {
            manager: channel,
            migration: startup.migration,
            mapping: Some((h, va, ECAM_SIZE)),
            requests: 0,
        },
        registry,
        cursor: bus.allocation_cursor(),
        root,
        next_poll_ms: 0,
        known,
        ports,
        pending: None,
        power: Vec::new(),
        device_controls,
        extended: true,
    };
    state.validate().expect("PCI runtime state");
    runtime::serve(state);
}
