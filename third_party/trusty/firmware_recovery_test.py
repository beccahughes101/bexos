"""Reboot native signed firmware slots; every boot owns and reaps its helpers."""
import os
import shutil

SLOT_BYTES = 65 * 1024 * 1024
DISK_BYTES = (4096 + 4 * SLOT_BYTES + 16 * 1024 - 1) & ~(16 * 1024 - 1)


def create_disk(path):
    with path.open('xb') as output:
        output.truncate(DISK_BYTES)
        output.flush()
        os.fsync(output.fileno())


def run(args, image, rpmbd, template, state, work, disk, boot):
    firmware = work / 'firmware.raw'
    create_disk(firmware)
    markers = (
        'monitor-runtime: signed monitor inactive slot flushed authenticated and pending',
        'monitor-runtime: executed selected monitor trial committed through authenticated read',
        'monitor-runtime: executed committed monitor recovered from persistent firmware slot',
    ) if args.firmware_recovery else (
        'monitor-runtime: signed Trusty inactive slot flushed authenticated and pending',
        'monitor-runtime: disk-selected Trusty trial committed and rebooted into secure services',
        'monitor-runtime: committed distinct Trusty recovered and secure services continued',
    )
    for index, marker in enumerate(markers):
        required = [marker]
        if index == 1:
            required.append('monitor-runtime: committed trial acknowledgement deliberately withheld')
        if args.firmware_recovery and index > 0:
            required.append('monitor-runtime: real Trusty IPC continued after distinct monitor entry and old-image reuse')
        if args.trusty_recovery and index == 1:
            required.append('monitor-runtime: distinct Trusty trial code executed with persistent transport fenced')
        boot(image, rpmbd, state, work, required, args.timeout, args.memory_mib, disk, firmware)
    # The committed selection stays protected even if its image vanishes.
    with firmware.open('r+b') as output:
        output.seek(4096 + (3 if args.firmware_recovery else 1) * SLOT_BYTES)
        output.write(bytes(512))
        output.flush()
        os.fsync(output.fileno())
    boot(image, rpmbd, state, work,
         ['monitor-runtime: firmware recovery required; no older image admitted'],
         args.timeout, args.memory_mib, disk, firmware)
    # Independent installation: interrupt two published trial attempts at a
    # deterministic resident cut point, then require authenticated bounded abort.
    trial_work = work / 'interrupted-trials'
    trial_work.mkdir()
    trial_state = trial_work / 'RPMB_DATA'
    shutil.copyfile(template, trial_state)
    trial_firmware = trial_work / 'firmware.raw'
    create_disk(trial_firmware)
    boot(image, rpmbd, trial_state, trial_work,
         [markers[0]],
         args.timeout, args.memory_mib, None, trial_firmware)
    for _ in range(2):
        boot(image, rpmbd, trial_state, trial_work,
             ['monitor-runtime: authenticated reboot-selected monitor loaded from firmware disk' if args.firmware_recovery else
              'monitor-runtime: distinct Trusty trial code executed with persistent transport fenced'],
             args.timeout, args.memory_mib, None, trial_firmware, recovery_cut=1)
    boot(image, rpmbd, trial_state, trial_work,
         ['monitor-runtime: exhausted or invalid monitor trial rolled back authentically' if args.firmware_recovery else
          'monitor-runtime: exhausted Trusty trial rolled back with persistent service data intact'],
         args.timeout, args.memory_mib, None, trial_firmware)
    # Power loss after the protected transaction succeeded but before its
    # acknowledgement reaches the selection coordinator must boot committed.
    commit_work = work / 'interrupted-commit'
    commit_work.mkdir()
    commit_state = commit_work / 'RPMB_DATA'
    shutil.copyfile(template, commit_state)
    commit_firmware = commit_work / 'firmware.raw'
    create_disk(commit_firmware)
    boot(image, rpmbd, commit_state, commit_work, [markers[0]],
         args.timeout, args.memory_mib, None, commit_firmware)
    boot(image, rpmbd, commit_state, commit_work,
         ['monitor-runtime: committed trial acknowledgement deliberately withheld'],
         args.timeout, args.memory_mib, None, commit_firmware, recovery_cut=2)
    required = [markers[2]]
    if args.firmware_recovery:
        required.append('monitor-runtime: real Trusty IPC continued after distinct monitor entry and old-image reuse')
    boot(image, rpmbd, commit_state, commit_work, required,
         args.timeout, args.memory_mib, None, commit_firmware)
    # Each cut kills an actual guest using the resident ATA transport. Cuts
    # before publication must reinstall; a durable pending record must proceed
    # directly to its authenticated trial despite the lost acknowledgement.
    for cut, cut_marker in [
        (3, 'monitor-runtime: interrupted inactive slot has durable partial payload'),
        (5, 'monitor-runtime: authenticated inactive slot interrupted before pending publication'),
        (4, 'monitor-runtime: authenticated pending publication acknowledgement withheld'),
    ]:
        cut_work = work / f'interrupted-stage-{cut}'
        cut_work.mkdir()
        cut_state = cut_work / 'RPMB_DATA'
        shutil.copyfile(template, cut_state)
        cut_firmware = cut_work / 'firmware.raw'
        create_disk(cut_firmware)
        boot(image, rpmbd, cut_state, cut_work, [cut_marker],
             args.timeout, args.memory_mib, None, cut_firmware, recovery_cut=cut)
        if cut != 4:
            boot(image, rpmbd, cut_state, cut_work, [markers[0]],
                 args.timeout, args.memory_mib, None, cut_firmware)
        boot(image, rpmbd, cut_state, cut_work, [markers[1]],
             args.timeout, args.memory_mib, None, cut_firmware)
        boot(image, rpmbd, cut_state, cut_work, [markers[2]],
             args.timeout, args.memory_mib, None, cut_firmware)
