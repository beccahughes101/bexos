//! Functional GPU queue/readback probe for the nested Linux Venus fixture.
//! Software Vulkan validates submission and synchronization, not GPU speed.
use ash::vk;

fn verify_memory_lifecycle() -> Result<(), vk::Result> {
    let mut context = 0;
    let mut capset = [0u8; 4096];
    let status =
        unsafe { super::bexos_venus_open(&mut context, capset.as_mut_ptr().cast(), capset.len()) };
    if status != 0 {
        return Err(vk::Result::from_raw(status));
    }
    let mut memory = super::memory::Memory::default();
    let created =
        unsafe { super::memory::bexos_venus_memory_create(context, 4096, 0, 1, &mut memory) };
    let released = if created == 0 {
        bexos_userspace::log("input-fixture: Venus lifecycle mapping created\n");
        unsafe { super::memory::bexos_venus_memory_release(context, &mut memory) }
    } else {
        created
    };
    let closed = super::bexos_venus_close(context);
    if released != 0 || closed != 0 {
        bexos_userspace::log(&format!(
            "input-fixture: Venus lifecycle failed create={created} release={released} close={closed}\n"
        ));
        return Err(vk::Result::ERROR_DEVICE_LOST);
    }
    bexos_userspace::log("input-fixture: Venus mapping release/context destruction verified\n");
    Ok(())
}
struct Instance(ash::Instance);
impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            self.0.destroy_instance(None);
        }
    }
}
struct Device(ash::Device);
impl Drop for Device {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.device_wait_idle();
            self.0.destroy_device(None);
        }
    }
}
struct Work<'a> {
    device: &'a ash::Device,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    pool: vk::CommandPool,
    fence: vk::Fence,
    mapped: bool,
}
impl Drop for Work<'_> {
    fn drop(&mut self) {
        bexos_userspace::log("input-fixture: Mesa buffer probe cleanup beginning\n");
        unsafe {
            let _ = self.device.device_wait_idle();
            if self.mapped {
                self.device.unmap_memory(self.memory);
            }
            self.device.destroy_fence(self.fence, None);
            self.device.destroy_command_pool(self.pool, None);
            self.device.destroy_buffer(self.buffer, None);
            self.device.free_memory(self.memory, None);
        }
        bexos_userspace::log("input-fixture: Mesa buffer probe cleanup complete\n");
    }
}
pub fn verify_submission() -> Result<(), vk::Result> {
    verify_memory_lifecycle()?;
    unsafe {
        unsafe extern "C" {
            fn bexos_venus_enable_diagnostics();
        }
        bexos_venus_enable_diagnostics();
        let entry = super::entry();
        let app = vk::ApplicationInfo::default()
            .application_name(c"BexOS Venus fixture")
            .api_version(vk::API_VERSION_1_1);
        let instance = Instance(entry.create_instance(
            &vk::InstanceCreateInfo::default().application_info(&app),
            None,
        )?);
        bexos_userspace::log("input-fixture: Mesa Vulkan instance created\n");
        let (physical, family) = instance
            .0
            .enumerate_physical_devices()?
            .into_iter()
            .find_map(|p| {
                instance
                    .0
                    .get_physical_device_queue_family_properties(p)
                    .iter()
                    .position(|q| {
                        q.queue_count != 0 && q.queue_flags.contains(vk::QueueFlags::GRAPHICS)
                    })
                    .map(|q| (p, q as u32))
            })
            .ok_or(vk::Result::ERROR_INITIALIZATION_FAILED)?;
        let priorities = [1.];
        let queues = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(family)
            .queue_priorities(&priorities)];
        let device = Device(instance.0.create_device(
            physical,
            &vk::DeviceCreateInfo::default().queue_create_infos(&queues),
            None,
        )?);
        bexos_userspace::log("input-fixture: Mesa Vulkan device created\n");
        let mut work = Work {
            device: &device.0,
            buffer: vk::Buffer::null(),
            memory: vk::DeviceMemory::null(),
            pool: vk::CommandPool::null(),
            fence: vk::Fence::null(),
            mapped: false,
        };
        work.buffer = device.0.create_buffer(
            &vk::BufferCreateInfo::default()
                .size(4096)
                .usage(vk::BufferUsageFlags::TRANSFER_DST)
                .sharing_mode(vk::SharingMode::EXCLUSIVE),
            None,
        )?;
        let requirements = device.0.get_buffer_memory_requirements(work.buffer);
        bexos_userspace::log(&format!(
            "input-fixture: Mesa buffer created allocation_bytes={}\n",
            requirements.size
        ));
        let properties = instance.0.get_physical_device_memory_properties(physical);
        let memory_type = (0..properties.memory_type_count)
            .find(|i| {
                requirements.memory_type_bits & (1 << i) != 0
                    && properties.memory_types[*i as usize]
                        .property_flags
                        .contains(
                            vk::MemoryPropertyFlags::HOST_VISIBLE
                                | vk::MemoryPropertyFlags::HOST_COHERENT,
                        )
            })
            .ok_or(vk::Result::ERROR_FEATURE_NOT_PRESENT)?;
        work.memory = device.0.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(memory_type),
            None,
        )?;
        bexos_userspace::log("input-fixture: Mesa device memory allocated\n");
        device.0.bind_buffer_memory(work.buffer, work.memory, 0)?;
        let mapped = device
            .0
            .map_memory(work.memory, 0, 4096, vk::MemoryMapFlags::empty())?;
        work.mapped = true;
        bexos_userspace::log("input-fixture: Mesa device memory mapped\n");
        core::ptr::write_bytes(mapped.cast::<u8>(), 0, 4096);
        work.pool = device.0.create_command_pool(
            &vk::CommandPoolCreateInfo::default().queue_family_index(family),
            None,
        )?;
        let command = device.0.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(work.pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1),
        )?[0];
        device.0.begin_command_buffer(
            command,
            &vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
        )?;
        device
            .0
            .cmd_fill_buffer(command, work.buffer, 0, 4096, 0xa18f_cafe);
        let barriers = [vk::MemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::HOST_READ)];
        device.0.cmd_pipeline_barrier(
            command,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::HOST,
            vk::DependencyFlags::empty(),
            &barriers,
            &[],
            &[],
        );
        device.0.end_command_buffer(command)?;
        bexos_userspace::log("input-fixture: Mesa fill command recorded\n");
        work.fence = device
            .0
            .create_fence(&vk::FenceCreateInfo::default(), None)?;
        let commands = [command];
        let submits = [vk::SubmitInfo::default().command_buffers(&commands)];
        device
            .0
            .queue_submit(device.0.get_device_queue(family, 0), &submits, work.fence)?;
        bexos_userspace::log("input-fixture: Mesa queue submitted\n");
        device
            .0
            .wait_for_fences(&[work.fence], true, 5_000_000_000)?;
        bexos_userspace::log("input-fixture: Mesa Vulkan fence completed\n");
        for i in 0..1024 {
            if core::ptr::read_volatile(mapped.cast::<u32>().add(i)) != 0xa18f_cafe {
                return Err(vk::Result::ERROR_DEVICE_LOST);
            }
        }
        bexos_userspace::log("input-fixture: Mesa Vulkan readback verified\n");
        Ok(())
    }
}
