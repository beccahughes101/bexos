"""Vulkan 1.1 queue/fence/readback check, independent of window systems.

Uses the public Vulkan C ABI. The fixture selects an ICD explicitly. This
checks actual command execution, rather than only physical-device enumeration.
"""
import ctypes as c

U32, U64, PTR = c.c_uint32, c.c_uint64, c.c_void_p


def structure(name, *fields):
    return type(name, (c.Structure,), {"_fields_": [("sType", U32), ("pNext", PTR), *fields]})


Application = structure("Application", ("name", PTR), ("version", U32), ("engine", PTR),
                        ("engineVersion", U32), ("apiVersion", U32))
Instance = structure("Instance", ("flags", U32), ("application", PTR), ("layerCount", U32),
                     ("layers", PTR), ("extensionCount", U32), ("extensions", PTR))
QueueCreate = structure("QueueCreate", ("flags", U32), ("family", U32), ("count", U32), ("priorities", PTR))
DeviceCreate = structure("DeviceCreate", ("flags", U32), ("queueCount", U32), ("queues", PTR),
                         ("layerCount", U32), ("layers", PTR), ("extensionCount", U32),
                         ("extensions", PTR), ("features", PTR))
BufferCreate = structure("BufferCreate", ("flags", U32), ("size", U64), ("usage", U32),
                         ("sharing", U32), ("familyCount", U32), ("families", PTR))
MemoryAllocate = structure("MemoryAllocate", ("size", U64), ("memoryType", U32))
PoolCreate = structure("PoolCreate", ("flags", U32), ("family", U32))
CommandAllocate = structure("CommandAllocate", ("pool", PTR), ("level", U32), ("count", U32))
CommandBegin = structure("CommandBegin", ("flags", U32), ("inheritance", PTR))
FenceCreate = structure("FenceCreate", ("flags", U32))
MemoryBarrier = structure("MemoryBarrier", ("sourceAccess", U32), ("destinationAccess", U32))
Submit = structure("Submit", ("waitCount", U32), ("waits", PTR), ("waitStages", PTR),
                   ("commandCount", U32), ("commands", PTR), ("signalCount", U32), ("signals", PTR))


class QueueProperties(c.Structure):
    _fields_ = [("flags", U32), ("count", U32), ("timestampBits", U32), ("granularity", U32 * 3)]


class MemoryType(c.Structure):
    _fields_ = [("flags", U32), ("heap", U32)]


class MemoryHeap(c.Structure):
    _fields_ = [("size", U64), ("flags", U32)]


class MemoryProperties(c.Structure):
    _fields_ = [("typeCount", U32), ("types", MemoryType * 32), ("heapCount", U32), ("heaps", MemoryHeap * 16)]


class Requirements(c.Structure):
    _fields_ = [("size", U64), ("alignment", U64), ("typeBits", U32)]


def address(value):
    return c.cast(c.byref(value), PTR)


def probe():
    vk = c.CDLL("libvulkan.so.1")
    signatures = {
        "CreateInstance": [PTR, PTR, PTR], "DestroyInstance": [PTR, PTR],
        "EnumeratePhysicalDevices": [PTR, PTR, PTR],
        "GetPhysicalDeviceQueueFamilyProperties": [PTR, PTR, PTR],
        "GetPhysicalDeviceMemoryProperties": [PTR, PTR],
        "CreateDevice": [PTR, PTR, PTR, PTR], "DestroyDevice": [PTR, PTR],
        "GetDeviceQueue": [PTR, U32, U32, PTR], "DeviceWaitIdle": [PTR],
        "CreateBuffer": [PTR, PTR, PTR, PTR], "DestroyBuffer": [PTR, PTR, PTR],
        "GetBufferMemoryRequirements": [PTR, PTR, PTR], "AllocateMemory": [PTR, PTR, PTR, PTR],
        "FreeMemory": [PTR, PTR, PTR], "BindBufferMemory": [PTR, PTR, PTR, U64],
        "MapMemory": [PTR, PTR, U64, U64, U32, PTR], "UnmapMemory": [PTR, PTR],
        "CreateCommandPool": [PTR, PTR, PTR, PTR], "DestroyCommandPool": [PTR, PTR, PTR],
        "AllocateCommandBuffers": [PTR, PTR, PTR], "BeginCommandBuffer": [PTR, PTR],
        "EndCommandBuffer": [PTR], "CmdFillBuffer": [PTR, PTR, U64, U64, U32],
        "CmdPipelineBarrier": [PTR, U32, U32, U32, U32, PTR, U32, PTR, U32, PTR],
        "CreateFence": [PTR, PTR, PTR, PTR], "DestroyFence": [PTR, PTR, PTR],
        "QueueSubmit": [PTR, U32, PTR, PTR], "WaitForFences": [PTR, U32, PTR, U32, U64],
    }
    void = {"DestroyInstance", "GetPhysicalDeviceQueueFamilyProperties", "GetPhysicalDeviceMemoryProperties",
            "DestroyDevice", "GetDeviceQueue", "DestroyBuffer", "GetBufferMemoryRequirements", "FreeMemory",
            "UnmapMemory", "DestroyCommandPool", "CmdFillBuffer", "CmdPipelineBarrier", "DestroyFence"}
    for name, arguments in signatures.items():
        function = getattr(vk, "vk" + name)
        function.argtypes = arguments
        function.restype = None if name in void else c.c_int32

    def call(name, *arguments):
        result = getattr(vk, "vk" + name)(*arguments)
        if result not in (None, 0):
            raise RuntimeError(f"vk{name}: {result}")

    instance, device, buffer, memory, pool, fence, mapped = (PTR() for _ in range(7))
    try:
        app = Application(sType=0, apiVersion=(1 << 22) | (1 << 12))
        info = Instance(sType=1, application=address(app))
        call("CreateInstance", address(info), None, address(instance))
        count = U32()
        call("EnumeratePhysicalDevices", instance, address(count), None)
        assert 0 < count.value <= 16
        physical = (PTR * count.value)()
        call("EnumeratePhysicalDevices", instance, address(count), physical)
        gpu = physical[0]
        call("GetPhysicalDeviceQueueFamilyProperties", gpu, address(count), None)
        assert 0 < count.value <= 32
        families = (QueueProperties * count.value)()
        call("GetPhysicalDeviceQueueFamilyProperties", gpu, address(count), families)
        family = next(i for i, props in enumerate(families) if props.count and props.flags & 7)
        priority = c.c_float(1)
        queue_info = QueueCreate(sType=2, family=family, count=1, priorities=address(priority))
        device_info = DeviceCreate(sType=3, queueCount=1, queues=address(queue_info))
        call("CreateDevice", gpu, address(device_info), None, address(device))
        queue = PTR()
        call("GetDeviceQueue", device, family, 0, address(queue))
        buffer_info = BufferCreate(sType=12, size=4096, usage=2)
        call("CreateBuffer", device, address(buffer_info), None, address(buffer))
        requirements = Requirements()
        call("GetBufferMemoryRequirements", device, buffer, address(requirements))
        properties = MemoryProperties()
        call("GetPhysicalDeviceMemoryProperties", gpu, address(properties))
        assert properties.typeCount <= 32
        memory_type = next(i for i in range(properties.typeCount)
                           if requirements.typeBits & (1 << i) and properties.types[i].flags & 6 == 6)
        allocation = MemoryAllocate(sType=5, size=requirements.size, memoryType=memory_type)
        call("AllocateMemory", device, address(allocation), None, address(memory))
        call("BindBufferMemory", device, buffer, memory, 0)
        pool_info = PoolCreate(sType=39, family=family)
        call("CreateCommandPool", device, address(pool_info), None, address(pool))
        allocation = CommandAllocate(sType=40, pool=pool, count=1)
        command = PTR()
        call("AllocateCommandBuffers", device, address(allocation), address(command))
        begin = CommandBegin(sType=42, flags=1)
        call("BeginCommandBuffer", command, address(begin))
        pattern = 0x1234ABCD
        call("CmdFillBuffer", command, buffer, 0, 4096, pattern)
        barrier = MemoryBarrier(sType=46, sourceAccess=0x1000, destinationAccess=0x2000)
        call("CmdPipelineBarrier", command, 0x1000, 0x4000, 0, 1, address(barrier), 0, None, 0, None)
        call("EndCommandBuffer", command)
        fence_info = FenceCreate(sType=8)
        call("CreateFence", device, address(fence_info), None, address(fence))
        submission = Submit(sType=4, commandCount=1, commands=address(command))
        call("QueueSubmit", queue, 1, address(submission), fence)
        call("WaitForFences", device, 1, address(fence), 1, 10_000_000_000)
        call("MapMemory", device, memory, 0, 4096, 0, address(mapped))
        assert list((U32 * 1024).from_address(mapped.value)) == [pattern] * 1024
        print("VULKAN_QUEUE_FENCE_READBACK_VERIFIED", flush=True)
    finally:
        if device:
            vk.vkDeviceWaitIdle(device)
            if mapped: vk.vkUnmapMemory(device, memory)
            if fence: vk.vkDestroyFence(device, fence, None)
            if pool: vk.vkDestroyCommandPool(device, pool, None)
            if buffer: vk.vkDestroyBuffer(device, buffer, None)
            if memory: vk.vkFreeMemory(device, memory, None)
            vk.vkDestroyDevice(device, None)
        if instance: vk.vkDestroyInstance(instance, None)
