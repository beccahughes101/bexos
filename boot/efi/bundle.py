"""Explicit Bazel refresh and validation of integrated x86 boot firmware."""
import importlib.util
import os
from pathlib import Path
import platform
import struct
import sys
import tarfile

NAMES = ('loader.efi', 'monitor.elf', 'trusty.elf', 'OVMF_CODE.fd',
         'OVMF_VARS.fd', 'boot_root.avbpubkey', 'rpmb_dev', 'RPMB_DATA',
         'rollback_loader.efi', 'revoked.fd', 'development.pem',
         'trusty.replacement.fw', 'hypervisor.replacement.fw', 'trusty.candidate.elf',
         'monitor.policy.elf', 'monitor.fault.elf', 'monitor.hang.elf', 'monitor.successor.elf',
         'hypervisor.fault.fw', 'hypervisor.hang.fw', 'hypervisor.successor.fw',
         'hypervisor.successor_fault.fw')


def header(variant):
    if variant not in ('standard', 'acceptance'):
        raise ValueError('unknown integrated firmware variant')
    return (f'format_version: 3\narchitecture: "x86_64"\nvariant: "{variant}"\n'
            f'host: "{platform.system()}-{platform.machine()}"\n'
            'monitor_abi_version: 1\nmonitor_architecture_id: 2\nboot_handoff_version: 4\n'
            'replacement_state_version: 2\n')


def validate(files):
    if set(files) != set(NAMES):
        raise ValueError('incomplete integrated firmware closure')
    for name in ('monitor.elf', 'trusty.elf', 'trusty.candidate.elf', 'monitor.policy.elf', 'monitor.fault.elf', 'monitor.hang.elf', 'monitor.successor.elf'):
        elf = files[name]
        if (len(elf) < 64 or elf[:7] != b'\x7fELF\x02\x01\x01'
                or struct.unpack_from('<HH', elf, 16) != (2, 62)):
            raise ValueError(f'{name}: wrong firmware architecture or ELF type')
    for name in ('loader.efi', 'rollback_loader.efi'):
        signed_loader(files[name])
        if (files['trusty.elf'] not in files[name]
                or files['boot_root.avbpubkey'] not in files[name]):
            raise ValueError(f'{name}: firmware trust inputs differ')
    if files['loader.efi'] == files['rollback_loader.efi']:
        raise ValueError('rollback fixture must be distinct from product firmware')
    if (files['monitor.elf'] not in files['loader.efi']
            or files['trusty.elf'] not in files['monitor.elf']
            or len(files['boot_root.avbpubkey']) != 520
            or files['boot_root.avbpubkey'] not in files['monitor.elf']):
        raise ValueError('firmware payload closure mismatch')
    for component, image, envelope in [
            ('trusty', 'trusty.candidate.elf', 'trusty.replacement.fw'),
            ('hypervisor', 'monitor.policy.elf', 'hypervisor.replacement.fw'),
            ('hypervisor', 'monitor.fault.elf', 'hypervisor.fault.fw'),
            ('hypervisor', 'monitor.hang.elf', 'hypervisor.hang.fw'),
            ('hypervisor', 'monitor.successor.elf', 'hypervisor.successor.fw'),
            ('hypervisor', 'monitor.fault.elf', 'hypervisor.successor_fault.fw')]:
        replacement = files[envelope]
        if len(replacement) < 32 or replacement[:8] != b'BEXFW001':
            raise ValueError('invalid replacement envelope')
        metadata_len, image_len, reserved = struct.unpack_from('<QQQ', replacement, 8)
        if (reserved or not 256 <= metadata_len <= 65536 or image_len != len(files[image])
                or 32 + metadata_len + image_len != len(replacement)
                or replacement[32 + metadata_len:] != files[image]):
            raise ValueError('replacement image differs from firmware closure')
        metadata = replacement[32:32 + metadata_len]
        if (files['boot_root.avbpubkey'] not in metadata
                or (component + '/x86_64/state/1').encode() not in metadata):
            raise ValueError('replacement trust inputs differ')


def signed_loader(loader):
    if len(loader) < 64 or loader[:2] != b'MZ':
        raise ValueError('invalid EFI loader')
    pe = struct.unpack_from('<I', loader, 60)[0]
    if (pe + 24 + 160 > len(loader) or loader[pe:pe + 4] != b'PE\0\0'
            or struct.unpack_from('<H', loader, pe + 4)[0] != 0x8664
            or struct.unpack_from('<H', loader, pe + 24)[0] != 0x20b):
        raise ValueError('wrong EFI architecture or format')
    signature, length = struct.unpack_from('<II', loader, pe + 24 + 144)
    if (signature < pe + 24 + 160 or signature % 8 or length < 8
            or signature + length != len(loader)):
        raise ValueError('unsigned EFI loader')
    certificate_length, revision, certificate_type = struct.unpack_from('<IHH', loader, signature)
    if (certificate_length < 8 or (certificate_length + 7) & ~7 != length
            or revision != 0x200 or certificate_type != 2):
        raise ValueError('invalid EFI signature table')


def main():
    shared, operation, variant, destination, *sources = sys.argv[1:]
    spec = importlib.util.spec_from_file_location('firmware_archive', shared)
    archive = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(archive)
    manifest_header = header(variant)
    if operation == 'pack':
        if len(sources) != len(NAMES):
            raise ValueError('pack requires complete firmware artifact inventory')
        files = {name: Path(path).read_bytes() for name, path in zip(NAMES, sources)}
        archive.write(destination, files, manifest_header, validate)
    elif operation in ('extract', 'refresh'):
        if len(sources) != 4:
            raise ValueError('saved firmware and pinned Trusty/OVMF/root inputs required')
        files = archive.read(sources[0], NAMES, manifest_header)
        validate(files)
        for name, source in zip(('trusty.elf', 'OVMF_CODE.fd', 'boot_root.avbpubkey'), sources[1:]):
            if files[name] != Path(source).read_bytes():
                raise ValueError(f'{name}: saved firmware differs from selected pinned input')
        if operation == 'extract':
            archive.extract(destination, files, manifest_header)
        else:
            workspace = os.environ.get('BUILD_WORKSPACE_DIRECTORY')
            if not workspace:
                raise ValueError('refresh must run through bazel run')
            target = Path(workspace) / destination
            archive.write(target, files, manifest_header, validate)
            print(f'Saved integrated x86 firmware: {target}', flush=True)
    else:
        raise ValueError('unknown firmware bundle operation')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, tarfile.TarError) as error:
        variant = ' --//build/platforms:trusty_variant=acceptance' if len(sys.argv) > 3 and sys.argv[3] == 'acceptance' else ''
        sys.exit(f'Integrated firmware: {error}\nRefresh explicitly with bazel run -c opt --config=x86_64{variant} //boot/efi:refresh_firmware')
