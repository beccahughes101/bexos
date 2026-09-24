#![no_std]
extern crate alloc;

mod ethernet;

use alloc::{format, vec::Vec};
use bexos_intel_nic::{Controller, DeviceFamily, RegisterBank};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{
    Channel, HardwareResourceKind, Memory, Startup, StartupHardwareResource,
    live_migration::{Resource, Source, State},
    log,
    service_binding::{BoundServiceEndpoint, ServiceBinding},
};
use hardware_manager_fidl::{
    DriverLifecyclePrepareStopRequest, DriverLifecyclePrepareStopResponse, FidlDecode, FidlEncode,
    HandleRef, Status,
};

use crate::ethernet::EthernetRuntime;

const MMIO_RIGHTS: u32 = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MmioRegisters {
    base: u64,
    length: u64,
}

impl RegisterBank for MmioRegisters {
    fn read(&self, offset: u32) -> u32 {
        if u64::from(offset).saturating_add(4) > self.length {
            return 0;
        }
        // SAFETY: appd supplies an attenuated BAR VMO; `new` validates the
        // register window and every access is bounds checked above.
        unsafe { core::ptr::read_volatile((self.base + u64::from(offset)) as *const u32) }
    }

    fn write(&mut self, offset: u32, value: u32) {
        if u64::from(offset).saturating_add(4) > self.length {
            return;
        }
        // SAFETY: see `read`; accesses are naturally aligned 32-bit registers.
        unsafe {
            core::ptr::write_volatile((self.base + u64::from(offset)) as *mut u32, value);
        }
    }
}

pub fn run(channel: u64, family: DeviceFamily, driver_name: &'static str) -> ! {
    let control = Channel(channel);
    let startup = Startup::receive(control).expect("Intel NIC startup");
    if startup.migration_target {
        match bexos_userspace::live_migration::receive::<Runtime>(
            control,
            startup.migration_generation,
        ) {
            Ok(mut state) if state.family == family => {
                state.driver_name = driver_name;
                serve(state)
            }
            _ => bexos_userspace::exit(),
        }
    }

    let mmio = required(&startup.driver_resources, HardwareResourceKind::Mmio);
    let interrupt = required(&startup.driver_resources, HardwareResourceKind::Interrupt);
    let iommu = required(&startup.driver_resources, HardwareResourceKind::IommuDomain);
    let bus_control = required(&startup.driver_resources, HardwareResourceKind::BusControl);
    let base = Memory::map(mmio.handle, mmio.length, MMIO_RIGHTS).expect("Intel NIC BAR mapping");
    let registers = MmioRegisters {
        base,
        length: mmio.length,
    };
    let mut controller =
        Controller::discover(registers, family, bexos_intel_nic::DEFAULT_RING_SIZE)
            .expect("Intel NIC identity and MAC");
    controller.reset();
    let ethernet = EthernetRuntime::new(iommu.handle, controller.mac, &mut controller)
        .expect("Intel NIC DMA rings");
    Startup::ready(control).expect("Intel NIC ready");
    log(&format!(
        "{driver_name}: ready mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}\n",
        controller.mac[0],
        controller.mac[1],
        controller.mac[2],
        controller.mac[3],
        controller.mac[4],
        controller.mac[5]
    ));
    serve(Runtime {
        control,
        migration: startup.migration,
        lifecycle: startup.driver_lifecycle,
        family,
        driver_name,
        mmio_handle: mmio.handle,
        mmio_base: base,
        mmio_length: mmio.length,
        interrupt: interrupt.handle,
        iommu: iommu.handle,
        bus_control: bus_control.handle,
        controller: Some(controller),
        ethernet: Some(ethernet),
        checkpoint: None,
        ethernet_checkpoint: None,
        endpoints: Vec::new(),
    })
}

fn required(
    resources: &[StartupHardwareResource],
    kind: HardwareResourceKind,
) -> StartupHardwareResource {
    resources
        .iter()
        .find(|resource| resource.kind == kind)
        .cloned()
        .expect("required Intel NIC hardware resource")
}

pub struct Runtime {
    control: Channel,
    migration: Option<Channel>,
    lifecycle: Option<Channel>,
    family: DeviceFamily,
    driver_name: &'static str,
    mmio_handle: u64,
    mmio_base: u64,
    mmio_length: u64,
    interrupt: u64,
    iommu: u64,
    bus_control: u64,
    controller: Option<Controller<MmioRegisters>>,
    ethernet: Option<EthernetRuntime>,
    checkpoint: Option<Vec<u8>>,
    ethernet_checkpoint: Option<Vec<u8>>,
    endpoints: Vec<BoundServiceEndpoint>,
}

impl State for Runtime {
    fn empty() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            lifecycle: None,
            family: DeviceFamily::E1000e,
            driver_name: "intel-nic",
            mmio_handle: 0,
            mmio_base: 0,
            mmio_length: 0,
            interrupt: 0,
            iommu: 0,
            bus_control: 0,
            controller: None,
            ethernet: None,
            checkpoint: None,
            ethernet_checkpoint: None,
            endpoints: Vec::new(),
        }
    }

    fn keys(&self) -> Vec<u64> {
        alloc::vec![0, 1, 2]
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        let mut writer = Encoder::new();
        match key {
            0 => {
                writer.word(1);
                writer.word(self.control.0);
                writer.word(self.migration.map_or(0, |channel| channel.0));
                writer.word(self.lifecycle.map_or(0, |channel| channel.0));
                writer.word(match self.family {
                    DeviceFamily::E1000e => 1,
                    DeviceFamily::Igb => 2,
                });
                writer.word(self.mmio_handle);
                writer.word(self.mmio_base);
                writer.word(self.mmio_length);
                writer.word(self.interrupt);
                writer.word(self.iommu);
                writer.word(self.bus_control);
                writer.word(self.endpoints.len() as u64);
                for endpoint in &self.endpoints {
                    writer.word(endpoint.channel.0);
                    writer.word(endpoint.allowed_methods.len() as u64);
                    for ordinal in &endpoint.allowed_methods {
                        writer.word(*ordinal);
                    }
                }
            }
            1 => writer.bytes(
                &self
                    .controller
                    .as_ref()
                    .ok_or(Error::BadState)?
                    .checkpoint(),
            ),
            2 => writer.bytes(
                &self
                    .ethernet
                    .as_ref()
                    .ok_or(Error::BadState)?
                    .checkpoint(),
            ),
            _ => return Err(Error::InvalidData),
        }
        Ok(Some(writer.finish()))
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        let mut reader = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        match key {
            0 => {
                if reader.word()? != 1 {
                    return Err(Error::UnsupportedVersion);
                }
                self.control = Channel(reader.word()?);
                self.migration = nonzero_channel(reader.word()?);
                self.lifecycle = nonzero_channel(reader.word()?);
                self.family = match reader.word()? {
                    1 => DeviceFamily::E1000e,
                    2 => DeviceFamily::Igb,
                    _ => return Err(Error::InvalidData),
                };
                self.mmio_handle = reader.word()?;
                self.mmio_base = reader.word()?;
                self.mmio_length = reader.word()?;
                self.interrupt = reader.word()?;
                self.iommu = reader.word()?;
                self.bus_control = reader.word()?;
                self.endpoints.clear();
                for _ in 0..reader.count(64)? {
                    let channel = Channel(reader.word()?);
                    let mut methods = Vec::new();
                    for _ in 0..reader.count(64)? {
                        methods.push(reader.word()?);
                    }
                    self.endpoints
                        .push(BoundServiceEndpoint::new(channel, methods));
                }
            }
            1 => self.checkpoint = Some(reader.bytes(1024 * 1024)?.to_vec()),
            2 => self.ethernet_checkpoint = Some(reader.bytes(1024 * 1024)?.to_vec()),
            _ => return Err(Error::InvalidData),
        }
        reader.finish()
    }

    fn finish_adoption(&mut self) -> Result<(), Error> {
        self.validate()?;
        self.controller = Some(Controller::adopt(
            MmioRegisters {
                base: self.mmio_base,
                length: self.mmio_length,
            },
            self.checkpoint.as_deref().ok_or(Error::BadState)?,
        )?);
        self.ethernet = Some(EthernetRuntime::adopt(
            self.ethernet_checkpoint
                .as_deref()
                .ok_or(Error::BadState)?,
        )?);
        Ok(())
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0
            || self.migration.is_none()
            || self.mmio_handle == 0
            || self.mmio_base == 0
            || self.mmio_length < 0x6000
            || self.interrupt == 0
            || self.iommu == 0
            || self.bus_control == 0
            || (self.controller.is_none() && self.checkpoint.is_none())
            || (self.ethernet.is_none() && self.ethernet_checkpoint.is_none())
        {
            Err(Error::InvalidData)
        } else {
            Ok(())
        }
    }

    fn resources(&self) -> Vec<Resource> {
        let mut resources = alloc::vec![
            Resource::Handle(self.control.0),
            Resource::Mapping {
                handle: self.mmio_handle,
                offset: 0,
                va: self.mmio_base,
                size: self.mmio_length,
                rights: MMIO_RIGHTS,
            },
            Resource::Handle(self.interrupt),
            Resource::Handle(self.iommu),
            Resource::Handle(self.bus_control),
        ];
        if let Some(migration) = self.migration {
            resources.push(Resource::Handle(migration.0));
        }
        if let Some(lifecycle) = self.lifecycle {
            resources.push(Resource::Handle(lifecycle.0));
        }
        resources.extend(
            self.endpoints
                .iter()
                .map(|endpoint| Resource::Handle(endpoint.channel.0)),
        );
        if let Some(ethernet) = &self.ethernet {
            resources.extend(ethernet.resources());
        }
        resources
    }

    fn activated(&mut self, generation: u64) {
        if let Some(controller) = &mut self.controller {
            if let Some(ethernet) = &self.ethernet {
                let _ = controller.program_rings(ethernet.rx_dma(), ethernet.tx_dma());
            }
            if controller.started {
                controller.start();
            }
        }
        log(&format!(
            "{}: adopted generation={generation}\n",
            self.driver_name
        ));
    }
}

fn serve(mut state: Runtime) -> ! {
    let mut source = Source::new(state.migration);
    loop {
        if source.poll(&state).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        poll_lifecycle(&mut state, &mut source);
        if source.quiescing() {
            if let Some(controller) = &mut state.controller {
                controller.quiesce();
            }
            bexos_userspace::yield_now();
            continue;
        }
        if let Ok(message) = state.control.try_recv() {
            if let (Some(endpoint), Ok(metadata)) = (
                message.handles.first().copied(),
                core::str::from_utf8(&message.bytes),
            ) {
                if let Some(binding) = ServiceBinding::parse(metadata) {
                    if binding.protocol_is("Device") {
                        state.endpoints.push(BoundServiceEndpoint::new(
                            Channel(endpoint),
                            binding.method_ordinals,
                        ));
                    } else {
                        let _ = Memory::close(endpoint);
                    }
                    source.changed(0);
                } else {
                    let _ = Memory::close(endpoint);
                }
            }
        }
        ethernet::poll_endpoints(&mut state, &mut source);
        ethernet::poll_packets(&mut state, &mut source);
        bexos_userspace::yield_now();
    }
}

fn poll_lifecycle(state: &mut Runtime, source: &mut Source) {
    let Some(lifecycle) = state.lifecycle else {
        return;
    };
    let Ok(message) = lifecycle.try_recv() else {
        return;
    };
    let (_, request) = envelope(&message.bytes);
    let handles: Vec<_> = message
        .handles
        .iter()
        .map(|raw| HandleRef { raw: *raw })
        .collect();
    let status = if DriverLifecyclePrepareStopRequest::decode(request, &handles).is_ok() {
        if let Some(controller) = &mut state.controller {
            controller.quiesce();
        }
        source.changed(1);
        Status::Ok
    } else {
        Status::ErrInvalidArgs
    };
    let mut bytes = [0; 32];
    let mut response_handles = [HandleRef { raw: 0 }; 1];
    if let Ok(encoded) =
        (DriverLifecyclePrepareStopResponse { status }).encode(&mut bytes, &mut response_handles)
    {
        let _ = lifecycle.send(&bytes[..encoded.bytes], &[]);
    }
}

fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    if bytes.len() < 8 {
        return (0, &[]);
    }
    (
        u64::from_le_bytes(bytes[..8].try_into().unwrap()),
        &bytes[8..],
    )
}

fn nonzero_channel(raw: u64) -> Option<Channel> {
    (raw != 0).then_some(Channel(raw))
}
