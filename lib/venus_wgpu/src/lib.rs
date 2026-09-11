//! Builds wgpu on the explicitly linked, capability-backed Mesa Venus ICD.
//! The transport must remain installed until all instances, devices, queues,
//! and their resources are dropped. Presentation belongs to the display driver.
use ash::vk;
use wgpu::hal::vulkan;
mod blur_probe;
pub mod probe;
mod readback;
mod text_probe;

/// Device objects are process-local caches. Drop them before uninstalling the
/// transport or preparing migration; logical scene resources live elsewhere.
pub struct Device {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub errors: bexos_flatland_render::errors::GpuErrors,
}

impl Device {
    pub async fn new() -> Result<Self, String> {
        Self::with_timestamps(false).await
    }

    pub async fn with_timestamps(profile: bool) -> Result<Self, String> {
        let instance = instance()?;
        bexos_userspace::log("venus-wgpu: instance created\n");
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .await
            .map_err(|e| format!("Venus adapter: {e:?}"))?;
        bexos_userspace::log("venus-wgpu: adapter selected\n");
        let mut features =
            adapter.features() & (wgpu::Features::CLEAR_TEXTURE | wgpu::Features::PIPELINE_CACHE);
        if profile
            && adapter
                .features()
                .contains(bexos_flatland_render::timing::FEATURES)
        {
            features |= bexos_flatland_render::timing::FEATURES;
        }
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Flatland Venus"),
                required_features: features,
                // Multiple renderer contexts share the host-visible aperture.
                // Small retained blocks reduce unused space and fragmentation;
                // larger individual allocations can still use dedicated blocks
                // within the transport's 64 MiB resource admission limit.
                memory_hints: wgpu::MemoryHints::Manual {
                    suballocated_device_memory_block_size: (1 << 20)..(16 << 20),
                },
                ..Default::default()
            })
            .await
            .map_err(|e| format!("Venus device: {e:?}"))?;
        bexos_userspace::log("venus-wgpu: device created\n");
        let errors = bexos_flatland_render::errors::GpuErrors::install(&device);
        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            errors,
        })
    }
}

pub fn instance() -> Result<wgpu::Instance, String> {
    let entry = bexos_venus_transport::entry();
    let supported = unsafe { entry.try_enumerate_instance_version() }
        .map_err(|e| format!("Venus instance version: {e:?}"))?
        .unwrap_or(vk::API_VERSION_1_0);
    if supported < vk::API_VERSION_1_1 {
        return Err("Venus requires Vulkan 1.1 or newer".into());
    }
    let version = supported.min(vk::API_VERSION_1_3);
    let flags = wgpu::InstanceFlags::empty();
    let extensions = vulkan::Instance::desired_extensions(&entry, version, flags)
        .map_err(|e| format!("Venus instance extensions: {e:?}"))?;
    let names: Vec<_> = extensions.iter().map(|name| name.as_ptr()).collect();
    let application = vk::ApplicationInfo::default()
        .application_name(c"BexOS Flatland")
        .engine_name(c"Vello")
        .api_version(version);
    let raw = unsafe {
        entry.create_instance(
            &vk::InstanceCreateInfo::default()
                .application_info(&application)
                .enabled_extension_names(&names),
            None,
        )
    }
    .map_err(|e| format!("Venus instance creation: {e:?}"))?;
    // The entry, API version and exact enabled extension list are carried into
    // HAL together. HAL owns destruction; no dynamic Linux loader is involved.
    let hal = unsafe {
        vulkan::Instance::from_raw(
            entry,
            raw.clone(),
            version,
            0,
            None,
            extensions,
            flags,
            wgpu::MemoryBudgetThresholds::default(),
            false,
            None,
        )
    };
    match hal {
        Ok(hal) => Ok(unsafe { wgpu::Instance::from_hal::<vulkan::Api>(hal) }),
        Err(error) => {
            unsafe {
                raw.destroy_instance(None);
            }
            Err(format!("Venus HAL instance: {error:?}"))
        }
    }
}

pub mod worker;

mod composition_probe;

pub mod worker_probe;
