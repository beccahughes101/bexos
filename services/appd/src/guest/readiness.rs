use crate::{
    BusType, ConsumedService, DeviceNodeInfo, DeviceProperty, DeviceRegistry, DeviceRegistryError,
    DriverBinding, HardwareAccessTier, HardwareResourceKind, HardwareResourceLease,
    LaunchedProcess, Manifest, ReadinessError, ReadinessGate, RegisteredDeviceNode,
};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use bexos_kernel_core::ipc::Capability;
use bexos_userspace::{Channel, Memory, ServiceGrant, Startup, StartupHardwareResource, log};
use hardware_manager_fidl::{
    DeviceRegistryRegisterDeviceNodeRequest, DeviceRegistryRegisterDeviceNodeResponse,
    DeviceRegistryUnregisterDeviceNodeRequest, DeviceRegistryUnregisterDeviceNodeResponse,
    FidlDecode, FidlEncode, HandleRef, Status,
};
// An independently owned worker endpoint needs discovery as well as commands.
// Keep the compositor and privileged renderer fixture bootstrap routes identical.
const GPU_TRANSPORT_METHODS: &[u64] = &[7, 8, 9, 10, 11, 12, 13, 14, 15];
pub struct Gate {
    pub registry: DeviceRegistry,
    pub binding_node_id: u64,
    pub binding_mmio: u64,
    pub binding_bar_size: u64,
    pub binding_hardware_access: HardwareAccessTier,
    pub binding_resources: Vec<StartupHardwareResource>,
    pub pending_lifecycle: Vec<(u64, Channel)>,
    pub pci: Option<Channel>,
    pub pci_registry: Option<Channel>,
    pub rtc: Option<Channel>,
    /// Retains the architectural UART driver's lifecycle endpoint.
    pub diagnostic_uart: Option<Channel>,
    /// Private framed debug transport, separate from the diagnostic UART.
    pub debug_serial: Option<Channel>,
    pub nvme: Option<Channel>,
    pub virtio_net: Option<Channel>,
    pub xhci: Vec<Channel>,
    pub usbd: Option<Channel>,
    pub framebuffer: Option<(u64, u64)>,
    pub gpu: Option<Channel>,
    pub inputs: Vec<Channel>,
    pub splash: Option<Channel>,
    pub scened: Option<Channel>,
    pub rpmb: Option<Channel>,
    pub bexfs: Option<Channel>,
    pub user_bexfs: Option<Channel>,
    pub archivefs: Option<Channel>,
    pub memfs: Option<Channel>,
    pub diskimage: Option<Channel>,
    pub vfsd: Option<Channel>,
    pub debugd: Option<Channel>,
    pub traced: Option<Channel>,
    pub updated: Option<Channel>,
    pub trustd: Option<Channel>,
    pub teed: Option<Channel>,
    pub powerd: Option<Channel>,
    pub users: Option<Channel>,
    pub keychain: Option<Channel>,
    pub fontd: Option<Channel>,
    pub trust_tls_roots: Vec<u8>,
    pub trust_app_roots: Vec<u8>,
    pub pending_trace_producers: Vec<super::trace_registry::PendingTraceProducer>,
    pub services: Vec<super::state::ManagedService>,
    pub service_directory_bindings: Vec<crate::ServiceDirectoryBinding>,
}
impl Gate {
    pub fn new() -> Self {
        Self {
            registry: DeviceRegistry::new(),
            binding_node_id: 0,
            binding_mmio: 0,
            binding_bar_size: 0,
            binding_hardware_access: HardwareAccessTier::None,
            binding_resources: Vec::new(),
            pending_lifecycle: Vec::new(),
            pci: None,
            pci_registry: None,
            rtc: None,
            diagnostic_uart: None,
            debug_serial: None,
            nvme: None,
            virtio_net: None,
            xhci: Vec::new(),
            usbd: None,
            framebuffer: None,
            gpu: None,
            inputs: Vec::new(),
            splash: None,
            scened: None,
            rpmb: None,
            bexfs: None,
            user_bexfs: None,
            archivefs: None,
            memfs: None,
            diskimage: None,
            vfsd: None,
            debugd: None,
            traced: None,
            updated: None,
            trustd: None,
            teed: None,
            powerd: None,
            users: None,
            keychain: None,
            fontd: None,
            trust_tls_roots: Vec::new(),
            trust_app_roots: Vec::new(),
            pending_trace_producers: Vec::new(),
            services: Vec::new(),
            service_directory_bindings: Vec::new(),
        }
    }
}
impl ReadinessGate for Gate {
    fn wait_ready(&mut self, p: &LaunchedProcess<'_>) -> Result<(), ReadinessError> {
        let c = Channel(p.result.service_manager_handle.raw);
        let name = p.process_ref.manifest.package_name.as_str();
        let authority = match name {
            "bexos.service.updated" => 2,
            "bexos.service.teed" => 4,
            _ => 0,
        };
        if authority != 0 {
            bexos_userspace::migration::set_authority(p.result.process_handle.raw, authority)
                .map_err(|_| ReadinessError::NotReady)?;
        }
        if name == "bexos.service.teed" {
            pin_secure_monitor_thread(p.result.main_thread_handle.raw)
                .map_err(|_| ReadinessError::NotReady)?;
        }
        let mut pci_registry_server = None;
        let mut pci_registry_client = None;
        if name == "bexos.driver.pci_root" {
            let (driver, appd) = Channel::pair().map_err(|_| ReadinessError::NotReady)?;
            pci_registry_client = Some(driver);
            pci_registry_server = Some(appd);
        }
        let resources = if let Some(registry) = pci_registry_client {
            alloc::vec![registry.0]
        } else if name == "bexos.service.splashd" && self.framebuffer.is_some() {
            let (framebuffer, descriptor) = self.framebuffer.unwrap();
            alloc::vec![
                Memory::duplicate(framebuffer, 1 | 2 | 4 | 16 | 32)
                    .map_err(|_| ReadinessError::NotReady)?,
                Memory::duplicate(descriptor, 1 | 2 | 16 | 32)
                    .map_err(|_| ReadinessError::NotReady)?
            ]
        } else if self.binding_mmio != 0 {
            alloc::vec![self.binding_mmio]
        } else if name == "bexos.service.trustd" {
            if self.trust_tls_roots.is_empty() || self.trust_app_roots.is_empty() {
                return Err(ReadinessError::NotReady);
            }
            alloc::vec![
                Memory::from_bytes_with_flags(
                    &self.trust_tls_roots,
                    kernel_fidl::VmoFlags::CONTIGUOUS_PHYS.0,
                )
                .map_err(|_| ReadinessError::NotReady)?,
                Memory::from_bytes_with_flags(
                    &self.trust_app_roots,
                    kernel_fidl::VmoFlags::CONTIGUOUS_PHYS.0,
                )
                .map_err(|_| ReadinessError::NotReady)?,
            ]
        } else if name == "bexos.driver.debugd" {
            let serial = self.debug_serial.ok_or(ReadinessError::NotReady)?;
            let (client, server) = Channel::pair().map_err(|_| ReadinessError::NotReady)?;
            serial
                .send(b"bexos.serial.bind", &[server.0])
                .map_err(|_| ReadinessError::NotReady)?;
            alloc::vec![client.0]
        } else {
            Vec::new()
        };
        let (migration_client, migration_server) =
            Channel::pair().map_err(|_| ReadinessError::NotReady)?;
        let lifecycle_pair = if self.binding_node_id != 0 {
            Some(Channel::pair().map_err(|_| ReadinessError::NotReady)?)
        } else {
            None
        };
        let service_grants = self.service_grants_for(p.process_ref.manifest)?;
        let trace_producer = super::trace_registry::allocate_trace_producer(
            p.result.process_handle.raw,
            p.result.main_thread_handle.raw,
        )
        .map_err(|_| ReadinessError::NotReady)?;
        let pending_trace =
            super::trace_registry::PendingTraceProducer::from_allocation(name, &trace_producer);
        let send_result = if let Some((driver_lifecycle, appd_lifecycle)) = lifecycle_pair {
            self.pending_lifecycle
                .push((self.binding_node_id, appd_lifecycle));
            Startup::send_d1_driver(
                c,
                &self.binding_resources,
                driver_lifecycle,
                self.binding_node_id,
                Some(migration_server),
                0,
                false,
                &service_grants,
                p.result
                    .runtime_linker_data
                    .map(|(handle, len)| (handle.raw, len)),
                Some(trace_producer.startup),
            )
        } else {
            Startup::send_migratable_with_service_grants_config_linker_and_trace_data(
                c,
                &resources,
                if name == "bexos.service.trustd" {
                    self.trust_tls_roots.len() as u64
                } else {
                    self.binding_bar_size
                },
                if name == "bexos.service.trustd" {
                    self.trust_app_roots.len() as u64
                } else {
                    self.binding_node_id
                },
                "",
                Some(migration_server),
                0,
                false,
                &service_grants,
                None,
                p.result
                    .runtime_linker_data
                    .map(|(handle, len)| (handle.raw, len)),
                Some(trace_producer.startup),
            )
        };
        if send_result.is_err() {
            let _ = Memory::close(pending_trace.buffer);
            return Err(ReadinessError::PeerClosed);
        }
        // Startup must remain the first message on the service-manager channel:
        // teed decodes it before waiting for confirmation that appd installed
        // the secure-monitor authority on its process handle.
        if name == "bexos.service.teed" {
            c.send(b"bexos.authority.ready", &[])
                .map_err(|_| ReadinessError::NotReady)?;
        }
        match name {
            "bexos.driver.pci_root" => {
                let registry_channel = pci_registry_server.ok_or(ReadinessError::NotReady)?;
                let start = bexos_userspace::syscall::ticks();
                let timeout = bexos_userspace::syscall::frequency() * 60;
                loop {
                    match c.try_recv() {
                        Ok(m) if m.handles.is_empty() && m.bytes == 0i32.to_le_bytes() => break,
                        Ok(m)
                            if m.bytes == bexos_graphics_runtime::scheduling::PROFILE_MESSAGE
                                && m.handles.len() == 1
                                && matches!(
                                    name,
                                    "bexos.service.splashd" | "bexos.service.scened"
                                ) =>
                        {
                            if let Err(error) = super::graphics::apply_render_profile(
                                p.result.main_thread_handle.raw,
                                m.handles[0],
                            ) {
                                log(&alloc::format!(
                                    "appd: graphics profile attachment failed {error:?}\n"
                                ));
                            }
                        }
                        Ok(_) => return Err(ReadinessError::NotReady),
                        Err(kernel_fidl::Status::ErrTimedOut) => {}
                        Err(_) => return Err(ReadinessError::PeerClosed),
                    }
                    match registry_channel.try_recv() {
                        Ok(m) => {
                            serve_registry_request(
                                &mut self.registry,
                                registry_channel,
                                &m.bytes,
                                &m.handles,
                            )
                            .map_err(|_| ReadinessError::NotReady)?;
                        }
                        Err(kernel_fidl::Status::ErrTimedOut) => bexos_userspace::yield_now(),
                        Err(_) => return Err(ReadinessError::PeerClosed),
                    }
                    if bexos_userspace::syscall::ticks().wrapping_sub(start) > timeout {
                        return Err(ReadinessError::TimedOut);
                    }
                }
                self.pci = Some(c);
                self.pci_registry = Some(registry_channel);
            }
            "bexos.driver.rtc.pl031" | "bexos.driver.rtc.cmos" => self.rtc = Some(c),
            "bexos.driver.uart.pl011" => self.diagnostic_uart = Some(c),
            "bexos.driver.storage.nvme" => {
                self.nvme = Some(c);
            }
            "bexos.driver.network.virtio_net" => self.virtio_net = Some(c),
            "bexos.driver.usb.xhcid" => {
                self.xhci.push(c);
            }
            "bexos.driver.display.virtio_gpu" => self.gpu = Some(c),
            "bexos.driver.input.virtio" | "bexos.driver.input.usb_hid" => {
                if self.inputs.len() >= 16 {
                    return Err(ReadinessError::NotReady);
                }
                self.inputs.push(c);
                if let Some(scened) = self.scened {
                    let grant = self.notified_grant(
                        "bexos.hardware.input.InputDevice",
                        "InputDevice",
                        "Public",
                        &[1],
                        Some(c),
                    )?;
                    if scened
                        .send(
                            b"bexos.hardware.input.InputDevice|InputDevice|Public|1|",
                            &[grant.endpoint],
                        )
                        .is_err()
                    {
                        let _ = Memory::close(grant.endpoint);
                        return Err(ReadinessError::NotReady);
                    }
                }
            }
            "bexos.service.splashd" => self.splash = Some(c),
            "bexos.service.scened" => self.scened = Some(c),
            "bexos.service.usbd" => self.usbd = Some(c),
            "bexos.testing.input_fixture" => {}
            "bexos.driver.serial.virtio_console" => {
                Startup::wait_ready(c).map_err(|_| ReadinessError::TimedOut)?;
                c.send(b"bexos.serial.role", &[])
                    .map_err(|_| ReadinessError::PeerClosed)?;
                let role = c
                    .recv_with_timeout(120)
                    .map_err(|_| ReadinessError::TimedOut)?;
                if !role.handles.is_empty() {
                    for handle in role.handles {
                        let _ = Memory::close(handle);
                    }
                    return Err(ReadinessError::NotReady);
                }
                match role.bytes.as_slice() {
                    b"debug0" if self.debug_serial.is_none() => self.debug_serial = Some(c),
                    b"rpmb0" if self.rpmb.is_none() => self.rpmb = Some(c),
                    _ => return Err(ReadinessError::NotReady),
                }
            }
            "bexos.driver.storage.bexfs" => self.bexfs = Some(c),
            "bexos.driver.storage.user_bexfs" => self.user_bexfs = Some(c),
            "bexos.driver.storage.archivefs" => self.archivefs = Some(c),
            "bexos.driver.storage.memfs" => self.memfs = Some(c),
            "bexos.driver.storage.diskimage" => self.diskimage = Some(c),
            "bexos.service.vfsd" => self.vfsd = Some(c),
            "bexos.driver.debugd" => self.debugd = Some(c),
            "bexos.service.traced" => self.traced = Some(c),
            "bexos.service.updated" => self.updated = Some(c),
            "bexos.service.trustd" => self.trustd = Some(c),
            "bexos.service.teed" => self.teed = Some(c),
            "bexos.service.powerd" => self.powerd = Some(c),
            "bexos.service.usersd" => self.users = Some(c),
            "bexos.service.keychaind" => self.keychain = Some(c),
            "bexos.service.fontd" => self.fontd = Some(c),
            // Runtime-installed drivers are discovered from their manifest,
            // so their package names are intentionally not part of appd's
            // boot-time service-name table.  A nonzero binding node proves
            // this launch came through the authenticated device coordinator;
            // the generic ready handshake below is the commit point.
            _ if self.binding_node_id != 0 && p.process_ref.manifest.driver_info.is_some() => {}
            // Signed out-of-tree services do not have platform-reserved package
            // names. Their exact package/signer pair has already passed runner
            // policy before launch, and the signed manifest marks the process as
            // a service, so the ordinary ready handshake is their commit point.
            _ if p.process_ref.process.service => {}
            _ if p.process_ref.process.runner == "wasm" => {}
            _ => return Err(ReadinessError::NotReady),
        }
        if matches!(
            name,
            "bexos.service.splashd" | "bexos.service.scened" | "bexos.driver.display.virtio_gpu"
        ) {
            let deadline = bexos_graphics_runtime::now_us().saturating_add(3_000_000);
            loop {
                match c.try_recv() {
                    Ok(m) if m.handles.is_empty() && m.bytes == 0i32.to_le_bytes() => break,
                    Ok(m)
                        if m.bytes == bexos_graphics_runtime::scheduling::PROFILE_MESSAGE
                            && m.handles.len() == 1
                            && matches!(name, "bexos.service.splashd" | "bexos.service.scened") =>
                    {
                        if let Err(error) = super::graphics::apply_render_profile(
                            p.result.main_thread_handle.raw,
                            m.handles[0],
                        ) {
                            log(&alloc::format!(
                                "appd: graphics profile attachment failed {error:?}\n"
                            ));
                        }
                    }
                    Err(kernel_fidl::Status::ErrTimedOut)
                        if bexos_graphics_runtime::now_us() < deadline =>
                    {
                        bexos_userspace::yield_now();
                        continue;
                    }
                    _ => {
                        match name {
                            "bexos.service.splashd" => self.splash = None,
                            "bexos.service.scened" => self.scened = None,
                            _ => self.gpu = None,
                        }
                        self.binding_node_id = 0;
                        self.binding_mmio = 0;
                        self.binding_bar_size = 0;
                        self.binding_resources.clear();
                        let _ = Memory::close(migration_client.0);
                        let _ = Memory::close(pending_trace.buffer);
                        return Err(ReadinessError::TimedOut);
                    }
                }
            }
        } else if name != "bexos.driver.pci_root" && name != "bexos.driver.serial.virtio_console" {
            Startup::wait_ready(c).map_err(|_| ReadinessError::TimedOut)?;
        }
        self.pending_trace_producers.push(pending_trace);
        self.drain_trace_producers();
        self.binding_node_id = 0;
        self.binding_mmio = 0;
        self.binding_bar_size = 0;
        self.binding_hardware_access = HardwareAccessTier::None;
        self.binding_resources.clear();
        self.services.push(super::state::ManagedService {
            package: name.to_string(),
            process: p.process_ref.process.name.clone(),
            instance_id: String::new(),
            process_handle: p.result.process_handle.raw,
            space_handle: p.result.address_space_handle.raw,
            thread_handle: p.result.main_thread_handle.raw,
            manager: c.0,
            migration: migration_client.0,
            hardware: match p.hardware_access {
                HardwareAccessTier::None => 0,
                HardwareAccessTier::Isolated => 1,
                HardwareAccessTier::Direct => 2,
            },
            generation: 0,
            archive: 0,
            archive_len: 0,
            resource_group_id: 1,
            resource_job: 0,
        });
        log(&alloc::format!("appd: process ready package={name}\n"));
        Ok(())
    }

    fn registered_device_nodes(&self) -> &[RegisteredDeviceNode] {
        self.registry.nodes()
    }

    fn begin_device_binding(
        &mut self,
        node_id: u64,
        package_id: alloc::string::String,
        process_name: alloc::string::String,
        hardware_access: HardwareAccessTier,
    ) -> Result<(), DeviceRegistryError> {
        let failed_package = package_id.clone();
        let failed_process = process_name.clone();
        self.registry
            .begin_binding(node_id, package_id, process_name)?;
        if let Some(node) = self
            .registry
            .nodes()
            .iter()
            .find(|node| node.info.node_id == node_id)
        {
            self.binding_node_id = node_id;
            self.binding_hardware_access = hardware_access;
            self.binding_mmio = node.mmio_vmo.map_or(0, |cap| cap.object_id);
            self.binding_bar_size = node
                .resources
                .iter()
                .find(|resource| resource.kind == HardwareResourceKind::Mmio)
                .map_or(0, |resource| resource.length);
            self.binding_resources = crate::hardware_resources::duplicate_startup_resources(
                &node.resources,
                hardware_access,
                Memory::duplicate,
                |handle| {
                    let _ = Memory::close(handle);
                },
            )
            .map_err(|_| DeviceRegistryError::BadState(node_id))?;
            if let Some(control) = node
                .resources
                .iter()
                .find(|resource| resource.kind == HardwareResourceKind::BusControl)
                .map(|resource| resource.capability)
                && crate::lifecycle::set_pci_bus_master(control, true).is_err()
            {
                for resource in self.binding_resources.drain(..) {
                    let _ = Memory::close(resource.handle);
                }
                self.registry
                    .bind_failed(node_id, failed_package, failed_process)?;
                return Err(DeviceRegistryError::BadState(node_id));
            }
        }
        Ok(())
    }

    fn finish_device_binding(
        &mut self,
        node_id: u64,
        binding: DriverBinding,
    ) -> Result<(), DeviceRegistryError> {
        let lifecycle_channel = self
            .pending_lifecycle
            .iter()
            .position(|(pending_node_id, _)| *pending_node_id == node_id)
            .map(|index| self.pending_lifecycle.remove(index).1)
            .map(|channel| Capability {
                object_id: channel.0,
                rights: 0b11,
            });
        self.registry.bind_active(
            node_id,
            binding.process_handle,
            binding.manager_channel,
            lifecycle_channel,
        )
    }

    fn fail_device_binding(
        &mut self,
        node_id: u64,
        package_id: alloc::string::String,
        process_name: alloc::string::String,
    ) -> Result<(), DeviceRegistryError> {
        if let Some(control) = self
            .registry
            .nodes()
            .iter()
            .find(|node| node.info.node_id == node_id)
            .and_then(|node| {
                node.resources
                    .iter()
                    .find(|resource| resource.kind == HardwareResourceKind::BusControl)
            })
            .map(|resource| resource.capability)
        {
            let _ = crate::lifecycle::set_pci_bus_master(control, false);
            let _ = crate::lifecycle::reset_pci(control);
        }
        self.registry.bind_failed(node_id, package_id, process_name)
    }
}

pub(super) fn pin_secure_monitor_thread(thread: u64) -> Result<(), kernel_fidl::Status> {
    // Interrupted Trusty calls are CPU-local. Preserve the same sole caller
    // affinity at cold boot and before a replacement is allowed to activate.
    let response: kernel_fidl::TaskControlSetCpuAffinityResponse =
        bexos_userspace::ipc::kernel_call(
            3,
            "SetCpuAffinity",
            kernel_fidl::TASK_CONTROL_PUBLIC_METHODS,
            &kernel_fidl::TaskControlSetCpuAffinityRequest {
                thread: kernel_fidl::HandleRef { raw: thread },
                affinity: kernel_fidl::CpuMask { mask: 1 },
            },
        )?;
    if response.status == kernel_fidl::Status::Ok {
        Ok(())
    } else {
        Err(response.status)
    }
}

fn serve_registry_request(
    registry: &mut DeviceRegistry,
    reply: Channel,
    bytes: &[u8],
    handles: &[u64],
) -> Result<(), ()> {
    if bytes.len() < 8 {
        log("appd: device registry rejected a short request envelope\n");
        return Err(());
    }
    let ordinal = u64::from_le_bytes(bytes[..8].try_into().map_err(|_| {
        log("appd: device registry could not decode the request ordinal\n");
    })?);
    let request = &bytes[8..];
    let hs: Vec<_> = handles
        .iter()
        .map(|handle| HandleRef { raw: *handle })
        .collect();
    match ordinal {
        1 => {
            let request = DeviceRegistryRegisterDeviceNodeRequest::decode(request, &hs).map_err(
                |error| {
                    log(&alloc::format!(
                        "appd: device registry could not decode registration bytes={} handles={} error={error:?}\n",
                        request.len(),
                        hs.len()
                    ));
                },
            )?;
            let node_id = request.info.node_id;
            let status = match register_fidl_node(registry, request) {
                Ok(()) => Status::Ok,
                Err(error) => {
                    log(&alloc::format!(
                        "appd: device registry rejected node={} error={error:?}\n",
                        node_id
                    ));
                    status_from_registry_error(error)
                }
            };
            send_registry_response(reply, &DeviceRegistryRegisterDeviceNodeResponse { status })
                .map_err(|()| {
                    log("appd: device registry could not send registration response\n");
                })
        }
        2 => {
            let request =
                DeviceRegistryUnregisterDeviceNodeRequest::decode(request, &hs).map_err(|_| ())?;
            let status = match registry.unregister_device_node_post_order(request.node_id) {
                Ok(_) => Status::Ok,
                Err(error) => status_from_registry_error(error),
            };
            send_registry_response(
                reply,
                &DeviceRegistryUnregisterDeviceNodeResponse { status },
            )
        }
        _ => {
            log(&alloc::format!(
                "appd: device registry rejected unknown ordinal={ordinal}\n"
            ));
            Err(())
        }
    }
}

pub(super) fn register_fidl_node(
    registry: &mut DeviceRegistry,
    request: DeviceRegistryRegisterDeviceNodeRequest<'_>,
) -> Result<(), DeviceRegistryError> {
    let mut properties = Vec::new();
    for index in 0..request.info.properties.len() {
        let property = request
            .info
            .properties
            .get(index)
            .map_err(|_| DeviceRegistryError::BadState(request.info.node_id))?;
        properties.push(DeviceProperty {
            key: String::from(property.key),
            value: property.value,
        });
    }
    let mut resources = Vec::new();
    for index in 0..request.resources.len() {
        let resource = request
            .resources
            .get(index)
            .map_err(|_| DeviceRegistryError::BadState(request.info.node_id))?;
        resources.push(HardwareResourceLease {
            kind: match resource.kind {
                hardware_manager_fidl::HardwareResourceKind::Mmio => HardwareResourceKind::Mmio,
                hardware_manager_fidl::HardwareResourceKind::Interrupt => {
                    HardwareResourceKind::Interrupt
                }
                hardware_manager_fidl::HardwareResourceKind::DmaPool => {
                    HardwareResourceKind::DmaPool
                }
                hardware_manager_fidl::HardwareResourceKind::IommuDomain => {
                    HardwareResourceKind::IommuDomain
                }
                hardware_manager_fidl::HardwareResourceKind::RegisterProxy => {
                    HardwareResourceKind::RegisterProxy
                }
                hardware_manager_fidl::HardwareResourceKind::BusControl => {
                    HardwareResourceKind::BusControl
                }
            },
            resource_id: resource.resource_id,
            base: resource.base,
            length: resource.length,
            flags: resource.flags,
            capability: Capability {
                object_id: resource.resource.raw,
                rights: 1 | 2 | 4 | 16 | 32,
            },
        });
    }
    let bus = match request.info.bus {
        hardware_manager_fidl::BusType::Pci => BusType::Pci,
        hardware_manager_fidl::BusType::Usb => BusType::Usb,
        hardware_manager_fidl::BusType::PlatformDt => BusType::PlatformDt,
        hardware_manager_fidl::BusType::I2c => BusType::I2c,
        hardware_manager_fidl::BusType::Spi => BusType::Spi,
    };
    let mut allocated_domain = None;
    if bus == BusType::Pci
        && property(&properties, "pci.class") != Some(0x06)
        && !resources
            .iter()
            .any(|resource| resource.kind == HardwareResourceKind::IommuDomain)
    {
        let stream_id = pci_stream_id(&properties)
            .ok_or(DeviceRegistryError::BadState(request.info.node_id))?;
        let domain = Memory::create_iommu_domain(stream_id, 48)
            .map_err(|_| DeviceRegistryError::BadState(request.info.node_id))?;
        allocated_domain = Some(domain);
        resources.push(HardwareResourceLease {
            kind: HardwareResourceKind::IommuDomain,
            resource_id: 0x1_0000_0000 | stream_id,
            base: stream_id,
            length: 1,
            flags: 0,
            capability: Capability {
                object_id: domain,
                rights: 1 | 2 | 4 | 32,
            },
        });
    }
    let result = registry.register_device_node(RegisteredDeviceNode {
        info: DeviceNodeInfo {
            node_id: request.info.node_id,
            bus,
            parent_node_id: request
                .info
                .has_parent
                .then_some(request.info.parent_node_id),
            topological_path: request.info.topological_path.to_string(),
            properties,
        },
        resources,
        mmio_vmo: None,
        irq_channel: None,
        registrar: None,
        present: true,
        state: Default::default(),
    });
    if result.is_err() {
        if let Some(domain) = allocated_domain {
            let _ = Memory::close(domain);
        }
    }
    result
}

fn send_registry_response<R: FidlEncode>(reply: Channel, response: &R) -> Result<(), ()> {
    let mut bytes = [0; 256];
    let mut handles = [HandleRef { raw: 0 }; 2];
    let encoded = response.encode(&mut bytes, &mut handles).map_err(|_| ())?;
    reply
        .send(
            &bytes[..encoded.bytes],
            &handles[..encoded.handles]
                .iter()
                .map(|handle| handle.raw)
                .collect::<Vec<_>>(),
        )
        .map_err(|_| ())
}

fn status_from_registry_error(error: DeviceRegistryError) -> Status {
    match error {
        DeviceRegistryError::DuplicateNode(_)
        | DeviceRegistryError::DuplicateProperty(_)
        | DeviceRegistryError::DuplicateResource(_)
        | DeviceRegistryError::AlreadyBinding(_)
        | DeviceRegistryError::AlreadyActive(_) => Status::ErrAlreadyExists,
        DeviceRegistryError::AccessDenied(_) => Status::ErrAccessDenied,
        DeviceRegistryError::NotFound(_) | DeviceRegistryError::MissingParent(_) => {
            Status::ErrNotFound
        }
        DeviceRegistryError::EmptyPropertyKey
        | DeviceRegistryError::MissingResourceHandle(_)
        | DeviceRegistryError::InvalidParent(_)
        | DeviceRegistryError::BadState(_) => Status::ErrInvalidArgs,
    }
}

fn pci_stream_id(properties: &[DeviceProperty]) -> Option<u64> {
    let bus = property(properties, "pci.bus")?;
    let device = property(properties, "pci.device")?;
    let function = property(properties, "pci.function")?;
    if bus > 255 || device > 31 || function > 7 {
        return None;
    }
    Some((u64::from(bus) << 8) | (u64::from(device) << 3) | u64::from(function))
}

fn property(properties: &[DeviceProperty], key: &str) -> Option<u32> {
    properties
        .iter()
        .find(|property| property.key == key)
        .map(|property| property.value)
}

impl Gate {
    fn drain_trace_producers(&mut self) {
        let Some(traced) = self.traced else {
            return;
        };
        while let Some(pending) = self.pending_trace_producers.pop() {
            super::trace_registry::register_now(Some(traced), pending);
        }
    }

    fn service_grants_for(
        &mut self,
        manifest: &Manifest,
    ) -> Result<Vec<ServiceGrant>, ReadinessError> {
        let mut grants = Vec::new();
        match manifest.package_name.as_str() {
            "bexos.testing.input_fixture" => {
                grants.push(self.notified_grant(
                    "bexos.hardware.display.DisplayCoordinator",
                    "DisplayCoordinator",
                    "Public",
                    &[7, 8, 9],
                    self.gpu,
                )?);
                grants.push(self.notified_grant(
                    "bexos.hardware.display.DisplayCoordinator",
                    "DisplayCoordinator",
                    "GpuTransport",
                    GPU_TRANSPORT_METHODS,
                    self.gpu,
                )?);
                for _ in 0..2 {
                    grants.push(self.notified_grant(
                        "bexos.ui.scened.FlatlandSession",
                        "FlatlandSession",
                        "Public",
                        &[1, 2, 3, 4, 8, 9, 10, 14, 15, 16, 17, 18, 20, 21, 22, 23],
                        self.scened,
                    )?);
                }
                grants.push(self.notified_grant(
                    "bexos.ui.scened.AccessibilityControl",
                    "AccessibilityControl",
                    "Accessibility",
                    &[1, 2, 3, 4],
                    self.scened,
                )?);
                grants.push(self.notified_grant(
                    "bexos.ui.scened.ShellControl",
                    "ShellControl",
                    "Shell",
                    &[1, 2, 3, 4, 5],
                    self.scened,
                )?);
            }
            "bexos.app.dioxus_demo" => {
                if declares(manifest, "bexos.ui.scened.FlatlandSession") {
                    grants.push(self.notified_grant(
                        "bexos.ui.scened.FlatlandSession",
                        "FlatlandSession",
                        "Public",
                        &[1, 2, 8, 14, 15, 16, 20, 24],
                        self.scened,
                    )?);
                }
                if declares(manifest, "bexos.hardware.display.DisplayCoordinator")
                    && self.gpu.is_some()
                {
                    grants.push(self.notified_grant(
                        "bexos.hardware.display.DisplayCoordinator",
                        "DisplayCoordinator",
                        "GpuTransport",
                        GPU_TRANSPORT_METHODS,
                        self.gpu,
                    )?);
                }
            }
            "bexos.service.splashd" | "bexos.service.scened" => {
                if manifest.package_name == "bexos.service.scened" {
                    for input in &self.inputs {
                        grants.push(self.notified_grant(
                            "bexos.hardware.input.InputDevice",
                            "InputDevice",
                            "Public",
                            &[1],
                            Some(*input),
                        )?);
                    }
                    if declares(manifest, "bexos.fonts.FontProvider") {
                        grants.push(self.authenticated_system_grant(
                            "bexos.fonts.FontProvider",
                            "FontProvider",
                            "Public",
                            &[1, 2],
                            self.fontd,
                            &manifest.package_name,
                        )?);
                    }
                }
                if self.gpu.is_some() {
                    if let Ok(grant) = self.notified_grant(
                        "bexos.hardware.display.DisplayCoordinator",
                        "DisplayCoordinator",
                        "Public",
                        if manifest.package_name == "bexos.service.scened" {
                            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 16, 17, 18, 19]
                        } else {
                            &[1, 2, 3, 4, 5, 6, 7, 8, 9]
                        },
                        self.gpu,
                    ) {
                        grants.push(grant);
                    }
                }
                if manifest.package_name == "bexos.service.scened" && self.gpu.is_some() {
                    if let Ok(grant) = self.notified_grant(
                        "bexos.hardware.display.DisplayCoordinator",
                        "DisplayCoordinator",
                        "GpuTransport",
                        GPU_TRANSPORT_METHODS,
                        self.gpu,
                    ) {
                        grants.push(grant);
                    }
                }
                if manifest.package_name == "bexos.service.scened" && self.splash.is_some() {
                    if let Ok(grant) = self.notified_grant(
                        "bexos.splash.ProgressTracker",
                        "ProgressTracker",
                        "Public",
                        &[2],
                        self.splash,
                    ) {
                        grants.push(grant);
                    }
                }
            }
            "bexos.service.usersd" => {
                if declares(manifest, "bexos.service.vfsd") {
                    grants.push(self.notified_grant(
                        "bexos.service.vfsd",
                        "VfsManager",
                        "Public",
                        &[57, 58, 59, 60, 61],
                        self.vfsd,
                    )?);
                }
                if declares(manifest, "tee_manager") {
                    grants.push(self.notified_grant(
                        "tee_manager",
                        "TeeManager",
                        "Public",
                        &[5, 6, 7],
                        self.teed,
                    )?);
                }
            }
            "bexos.service.keychaind" => {
                if declares(manifest, "bexos.user.UserManager") {
                    grants.push(self.notified_grant(
                        "bexos.user.UserManager",
                        "UserManager",
                        "Public",
                        &[2],
                        self.users,
                    )?);
                    grants.push(self.notified_grant(
                        "bexos.user.UserManager",
                        "UserManager",
                        "UserAuthBroker",
                        &[9],
                        self.users,
                    )?);
                }
                if declares(manifest, "bexos.service.vfsd") {
                    grants.push(self.notified_grant(
                        "bexos.service.vfsd",
                        "VfsManager",
                        "Public",
                        &[61, 62],
                        self.vfsd,
                    )?);
                }
                if declares(manifest, "tee_manager") {
                    grants.push(self.notified_grant(
                        "tee_manager",
                        "TeeManager",
                        "Public",
                        &[5, 6, 7],
                        self.teed,
                    )?);
                }
            }
            "bexos.service.fontd" => {
                // Fontd starts before the storage-service broker loop. Retain the
                // directory peer in appd so remote misses can discover pkgd later.
                if declares(manifest, "ServiceDirectory") {
                    let (client, server) = Channel::pair().map_err(|_| ReadinessError::NotReady)?;
                    self.service_directory_bindings
                        .push(crate::ServiceDirectoryBinding {
                            channel: server.0,
                            package: manifest.package_name.clone(),
                            uid: 0,
                            system: true,
                            shell: false,
                        });
                    grants.push(ServiceGrant {
                        service: "ServiceDirectory".into(),
                        protocol: "bexos.app.service_directory.ServiceDirectory".into(),
                        capability: "Public".into(),
                        method_ordinals: alloc::vec![2],
                        permission_values: Vec::new(),
                        caller_package: None,
                        caller_uid: None,
                        caller_foreground: false,
                        provider_instance_id: None,
                        endpoint: client.0,
                    });
                }
                if declares(manifest, "bexos.user.UserManager") {
                    grants.push(self.notified_grant(
                        "bexos.user.UserManager",
                        "UserManager",
                        "Public",
                        &[2, 8],
                        self.users,
                    )?);
                }
                if declares(manifest, "bexos.service.vfsd") {
                    grants.push(self.notified_grant(
                        "bexos.service.vfsd",
                        "VfsManager",
                        "Public",
                        &[62],
                        self.vfsd,
                    )?);
                }
            }
            "bexos.driver.debugd" => {
                if declares(manifest, "bexos.tracing.TraceController") {
                    grants.push(self.notified_grant(
                        "bexos.tracing.TraceController",
                        "TraceController",
                        "Public",
                        &[1, 2, 3],
                        self.traced,
                    )?);
                }
                if self.teed.is_some() && declares(manifest, "tee_manager") {
                    grants.push(self.notified_grant(
                        "tee_manager",
                        "TeeManager",
                        "Public",
                        &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 13],
                        self.teed,
                    )?);
                }
            }
            "bexos.service.usbd" => {
                if let Some(xhci) = self.xhci.first().copied() {
                    grants.push(self.notified_grant(
                        "bexos.usb.host.XhciController",
                        "XhciController",
                        "UsbBusManager",
                        &[1, 2, 3, 4, 5, 6, 7, 8, 9],
                        Some(xhci),
                    )?);
                }
            }
            "bexos.service.updated" => {
                if self.teed.is_some() && declares(manifest, "tee_manager") {
                    grants.push(self.notified_grant(
                        "tee_manager",
                        "TeeManager",
                        "Public",
                        &[8, 9],
                        self.teed,
                    )?);
                }
            }
            _ => {}
        }
        Ok(grants)
    }

    pub(super) fn notified_grant(
        &self,
        service: &str,
        protocol: &str,
        capability: &str,
        method_ordinals: &[u64],
        manager: Option<Channel>,
    ) -> Result<ServiceGrant, ReadinessError> {
        let manager = manager.ok_or(ReadinessError::NotReady)?;
        let (client, provider) = Channel::pair().map_err(|_| ReadinessError::NotReady)?;
        let ordinals = join_ordinals(method_ordinals);
        let metadata = alloc::format!("{service}|{protocol}|{capability}|{ordinals}|");
        if manager.send(metadata.as_bytes(), &[provider.0]).is_err() {
            let _ = Memory::close(client.0);
            let _ = Memory::close(provider.0);
            return Err(ReadinessError::NotReady);
        }
        Ok(ServiceGrant {
            service: service.to_string(),
            protocol: protocol.to_string(),
            capability: capability.to_string(),
            method_ordinals: method_ordinals.to_vec(),
            permission_values: Vec::new(),
            caller_package: None,
            caller_uid: None,
            caller_foreground: false,
            provider_instance_id: None,
            endpoint: client.0,
        })
    }

    fn authenticated_system_grant(
        &self,
        service: &str,
        protocol: &str,
        capability: &str,
        method_ordinals: &[u64],
        manager: Option<Channel>,
        caller_package: &str,
    ) -> Result<ServiceGrant, ReadinessError> {
        let manager = manager.ok_or(ReadinessError::NotReady)?;
        let (client, provider) = Channel::pair().map_err(|_| ReadinessError::NotReady)?;
        let ordinals = join_ordinals(method_ordinals);
        let metadata =
            alloc::format!("{service}|{protocol}|{capability}|{ordinals}||{caller_package}|0|fg");
        if manager.send(metadata.as_bytes(), &[provider.0]).is_err() {
            let _ = Memory::close(client.0);
            let _ = Memory::close(provider.0);
            return Err(ReadinessError::NotReady);
        }
        Ok(ServiceGrant {
            service: service.to_string(),
            protocol: protocol.to_string(),
            capability: capability.to_string(),
            method_ordinals: method_ordinals.to_vec(),
            permission_values: Vec::new(),
            caller_package: Some(caller_package.to_string()),
            caller_uid: Some(0),
            caller_foreground: true,
            provider_instance_id: None,
            endpoint: client.0,
        })
    }
}

fn join_ordinals(method_ordinals: &[u64]) -> alloc::string::String {
    let mut out = alloc::string::String::new();
    for (index, ordinal) in method_ordinals.iter().enumerate() {
        if index != 0 {
            out.push(',');
        }
        out.push_str(&ordinal.to_string());
    }
    out
}

fn declares(manifest: &Manifest, service_name: &str) -> bool {
    manifest
        .services_consumed
        .iter()
        .any(|service: &ConsumedService| service.name == service_name)
}
