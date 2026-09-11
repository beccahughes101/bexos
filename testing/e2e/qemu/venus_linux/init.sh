#!/bin/busybox sh
/bin/busybox --install -s /bin
mount -t devtmpfs devtmpfs /dev
mount -t proc proc /proc
mount -t sysfs sysfs /sys
export PATH=/bin:/sbin:/usr/bin:/usr/sbin
export XDG_RUNTIME_DIR=/run
export LIBGL_ALWAYS_SOFTWARE=1
export EGL_PLATFORM=surfaceless
export VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.aarch64.json
if [ -e /fixture/bootfs ]; then
    modprobe virtio_blk
fi
python3 /fixture/check.py
status=$?
echo "VENUS_LINUX_FIXTURE_EXIT=$status"
sync
poweroff -f
