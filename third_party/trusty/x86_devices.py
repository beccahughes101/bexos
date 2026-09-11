"""Owned normal-world device fixtures for monitor integration tests."""
import os
import socket
import time


def arguments(disk, debug, rpmb=None):
    entropy = debug.with_suffix('.entropy')
    entropy.write_bytes(os.urandom(32))
    entropy.chmod(0o600)
    devices = ['-fw_cfg', f'name=opt/bexos/normal-entropy,file={entropy}', '-device', 'intel-iommu,intremap=on,eim=off',
            '-drive', f'if=none,id=normaldisk,file={disk},format=raw',
            '-device', 'nvme,addr=3,drive=normaldisk,serial=bexos-nvme0',
            '-chardev', f'socket,id=debug0,path={debug},server=on,wait=off',
            '-device', 'virtio-serial-pci,id=debugbus,addr=4,disable-legacy=on,disable-modern=off,iommu_platform=on',
            '-device', 'virtserialport,bus=debugbus.0,chardev=debug0,name=debug0,nr=1',
            '-netdev', 'user,id=normalnet',
            '-device', 'virtio-net-pci,addr=5,netdev=normalnet,disable-legacy=on,disable-modern=off,iommu_platform=on,mac=52:54:00:12:34:56']
    if rpmb is not None:
        devices += ['-chardev', f'socket,id=normalrpmb,path={rpmb},server=on,wait=off',
                    '-device', 'virtio-serial-pci,id=rpmbbus,addr=6,disable-legacy=on,disable-modern=off,iommu_platform=on',
                    '-device', 'virtserialport,bus=rpmbbus.0,chardev=normalrpmb,name=rpmb0,nr=1']
    return devices


def connect(debug, child=None):
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        deadline = time.monotonic() + 10
        while True:
            try:
                client.connect(str(debug))
                return client
            except (FileNotFoundError, ConnectionRefusedError):
                if time.monotonic() >= deadline or child is not None and child.poll() is not None:
                    raise RuntimeError('normal-world console did not become available')
                time.sleep(.01)
    except BaseException:
        client.close()
        raise
