use crate::vfs::{DescriptorSnapshot, Snapshot as VfsSnapshot};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_starnix_abi::{ABI_VERSION, NixRunnerOptions, UPSTREAM_REVISION};
use bexos_userspace::{
    Channel,
    live_migration::{Resource, State},
};
use std::vec::Vec;

const RECORD_VERSION: u64 = 2;
const MAX_MAPPINGS: usize = 96;
const MAX_REGISTER_BYTES: usize = 1024;
const MAX_RUNTIME_BYTES: usize = 64 * 1024;
const MAX_FDS: usize = 256;

pub(crate) struct RuntimeState {
    pub signals: Vec<u8>,
    pub dispatcher: Vec<u8>,
    pub vfs: VfsSnapshot,
    pub signal_frames: Vec<(u64, u64, u64)>,
}

impl RuntimeState {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut out = Encoder::new();
        out.word(1);
        out.bytes(&self.signals);
        out.bytes(&self.dispatcher);
        out.text(&self.vfs.cwd);
        out.word(self.vfs.descriptors.len() as u64);
        for (fd, descriptor) in &self.vfs.descriptors {
            out.word(u64::from(*fd));
            match descriptor {
                DescriptorSnapshot::Stdio { source } => {
                    out.word(0);
                    out.word(u64::from(*source));
                }
                DescriptorSnapshot::File {
                    path,
                    flags,
                    offset,
                } => {
                    out.word(1);
                    out.text(path);
                    out.word(u64::from(*flags));
                    out.word(*offset);
                }
                DescriptorSnapshot::Directory { path, flags } => {
                    out.word(2);
                    out.text(path);
                    out.word(u64::from(*flags));
                }
                DescriptorSnapshot::Null { flags } => {
                    out.word(3);
                    out.word(u64::from(*flags));
                }
                DescriptorSnapshot::Zero { flags } => {
                    out.word(4);
                    out.word(u64::from(*flags));
                }
            }
        }
        out.word(self.signal_frames.len() as u64);
        for (address, length, old_mask) in &self.signal_frames {
            out.word(*address);
            out.word(*length);
            out.word(*old_mask);
        }
        let bytes = out.finish();
        if bytes.len() > MAX_RUNTIME_BYTES {
            return Err(Error::Capacity);
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_RUNTIME_BYTES {
            return Err(Error::Capacity);
        }
        let mut input = Decoder::new(bytes);
        if input.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let signals = input.bytes(16 * 1024)?.to_vec();
        let dispatcher = input.bytes(16 * 1024)?.to_vec();
        let cwd = input.text(4096)?.into();
        let mut descriptors = Vec::new();
        for _ in 0..input.count(MAX_FDS)? {
            let fd = u16::try_from(input.word()?).map_err(|_| Error::InvalidData)?;
            let descriptor = match input.word()? {
                0 => {
                    let source = u8::try_from(input.word()?).map_err(|_| Error::InvalidData)?;
                    if source > 2 {
                        return Err(Error::InvalidData);
                    }
                    DescriptorSnapshot::Stdio { source }
                }
                1 => DescriptorSnapshot::File {
                    path: input.text(4096)?.into(),
                    flags: u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?,
                    offset: input.word()?,
                },
                2 => DescriptorSnapshot::Directory {
                    path: input.text(4096)?.into(),
                    flags: u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?,
                },
                3 => DescriptorSnapshot::Null {
                    flags: u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?,
                },
                4 => DescriptorSnapshot::Zero {
                    flags: u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?,
                },
                _ => return Err(Error::InvalidData),
            };
            if descriptors.iter().any(|(old, _)| *old == fd) {
                return Err(Error::InvalidData);
            }
            descriptors.push((fd, descriptor));
        }
        let mut signal_frames = Vec::new();
        for _ in 0..input.count(64)? {
            signal_frames.push((input.word()?, input.word()?, input.word()?));
        }
        input.finish()?;
        Ok(Self {
            signals,
            dispatcher,
            vfs: VfsSnapshot { cwd, descriptors },
            signal_frames,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Mapping {
    pub handle: u64,
    pub address: u64,
    pub size: u64,
    pub offset: u64,
    pub rights: u32,
    pub owned: bool,
}

pub(crate) struct Snapshot {
    architecture: u64,
    options: Vec<u8>,
    registers: Vec<u8>,
    runtime: Vec<u8>,
    pub mappings: Vec<Mapping>,
    migration: Option<Channel>,
    valid: bool,
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        for mapping in self.mappings.drain(..).filter(|mapping| mapping.owned) {
            let _ = bexos_userspace::Memory::close(mapping.handle);
        }
    }
}

impl Snapshot {
    pub fn source(
        architecture: u64,
        options: &NixRunnerOptions,
        registers: &[u8],
        runtime: Vec<u8>,
        mappings: Vec<Mapping>,
        migration: Option<Channel>,
    ) -> Result<Self, Error> {
        let value = Self {
            architecture,
            options: options.encode().map_err(|_| Error::InvalidData)?,
            registers: registers.to_vec(),
            runtime,
            mappings,
            migration,
            valid: true,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn candidate(architecture: u64, options: &NixRunnerOptions) -> Result<Self, Error> {
        Ok(Self {
            architecture,
            options: options.encode().map_err(|_| Error::InvalidData)?,
            registers: Vec::new(),
            runtime: Vec::new(),
            mappings: Vec::new(),
            migration: None,
            valid: false,
        })
    }

    pub fn capture_registers(&mut self, registers: &[u8]) -> Result<(), Error> {
        if registers.len() > MAX_REGISTER_BYTES {
            return Err(Error::Capacity);
        }
        self.registers.clear();
        self.registers.extend_from_slice(registers);
        self.valid = true;
        Ok(())
    }

    pub fn replace_mappings(&mut self, mappings: Vec<Mapping>) {
        for mapping in self.mappings.drain(..).filter(|mapping| mapping.owned) {
            let _ = bexos_userspace::Memory::close(mapping.handle);
        }
        self.mappings = mappings;
    }

    pub fn capture_runtime(&mut self, runtime: Vec<u8>) -> Result<(), Error> {
        if runtime.is_empty() || runtime.len() > MAX_RUNTIME_BYTES {
            return Err(Error::Capacity);
        }
        self.runtime = runtime;
        Ok(())
    }

    pub fn runtime(&self) -> &[u8] {
        &self.runtime
    }

    pub fn registers(&self) -> &[u8] {
        &self.registers
    }

    pub fn migration(&self) -> Option<Channel> {
        self.migration
    }
}

impl State for Snapshot {
    fn empty() -> Self {
        Self {
            architecture: u64::MAX,
            options: Vec::new(),
            registers: Vec::new(),
            runtime: Vec::new(),
            mappings: Vec::new(),
            migration: None,
            valid: false,
        }
    }

    fn keys(&self) -> Vec<u64> {
        vec![0]
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        if key != 0 || !self.valid {
            return Err(Error::InvalidData);
        }
        let mut out = Encoder::new();
        out.word(RECORD_VERSION);
        out.word(u64::from(ABI_VERSION));
        out.text(UPSTREAM_REVISION);
        out.word(self.architecture);
        out.bytes(&self.options);
        out.bytes(&self.registers);
        out.bytes(&self.runtime);
        out.word(self.migration.map_or(0, |channel| channel.0));
        out.word(self.mappings.len() as u64);
        for mapping in &self.mappings {
            out.word(mapping.handle);
            out.word(mapping.address);
            out.word(mapping.size);
            out.word(mapping.offset);
            out.word(u64::from(mapping.rights));
        }
        Ok(Some(out.finish()))
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key != 0 {
            return Err(Error::InvalidData);
        }
        let mut input = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        if input.word()? != RECORD_VERSION
            || input.word()? != u64::from(ABI_VERSION)
            || input.text(64)? != UPSTREAM_REVISION
            || input.word()? != self.architecture
        {
            return Err(Error::UnsupportedVersion);
        }
        if input.bytes(64 * 1024)? != self.options {
            return Err(Error::InvalidData);
        }
        self.registers = input.bytes(MAX_REGISTER_BYTES)?.to_vec();
        self.runtime = input.bytes(MAX_RUNTIME_BYTES)?.to_vec();
        let migration = input.word()?;
        self.migration = (migration != 0).then_some(Channel(migration));
        self.mappings.clear();
        for _ in 0..input.count(MAX_MAPPINGS)? {
            self.mappings.push(Mapping {
                handle: input.word()?,
                address: input.word()?,
                size: input.word()?,
                offset: input.word()?,
                rights: u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?,
                owned: false,
            });
        }
        input.finish()?;
        self.valid = true;
        self.validate()
    }

    fn validate(&self) -> Result<(), Error> {
        if !self.valid
            || self.registers.is_empty()
            || self.registers.len() > MAX_REGISTER_BYTES
            || self.runtime.is_empty()
            || self.runtime.len() > MAX_RUNTIME_BYTES
            || RuntimeState::decode(&self.runtime).is_err()
            || self.mappings.is_empty()
            || self.mappings.len() > MAX_MAPPINGS
            || self.mappings.iter().any(|mapping| {
                mapping.handle == 0
                    || mapping.size == 0
                    || mapping.address & 4095 != 0
                    || mapping.size & 4095 != 0
                    || mapping.offset & 4095 != 0
                    || mapping.rights & !0xe != 0
                    || mapping.rights & (4 | 8) == (4 | 8)
                    || mapping.address.checked_add(mapping.size).is_none_or(|end| {
                        mapping.address < bexos_boot::USER_START || end > bexos_boot::USER_END
                    })
            })
        {
            return Err(Error::InvalidData);
        }
        Ok(())
    }

    fn resources(&self) -> Vec<Resource> {
        self.mappings
            .iter()
            .map(|mapping| Resource::Mapping {
                handle: mapping.handle,
                offset: mapping.offset,
                va: mapping.address,
                size: mapping.size,
                rights: mapping.rights,
            })
            .collect()
    }

    fn activated(&mut self, _generation: u64) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_rejects_wrong_architecture_and_preserves_mapping_metadata() {
        let options = NixRunnerOptions {
            path: "/pkg/bin/looping".into(),
            arguments: vec!["looping".into()],
            environment: Vec::new(),
            rootfs: Default::default(),
        };
        let source = Snapshot::source(
            1,
            &options,
            &[7; 128],
            RuntimeState {
                signals: vec![1],
                dispatcher: vec![2],
                vfs: VfsSnapshot {
                    cwd: "/".into(),
                    descriptors: Vec::new(),
                },
                signal_frames: Vec::new(),
            }
            .encode()
            .unwrap(),
            vec![Mapping {
                handle: 9,
                address: 0x10_0000_0000,
                size: 4096,
                offset: 0,
                rights: 2 | 8,
                owned: false,
            }],
            Some(Channel(11)),
        )
        .unwrap();
        let bytes = source.encode_record(0).unwrap().unwrap();
        let mut candidate = Snapshot::candidate(1, &options).unwrap();
        candidate.adopt_record(0, Some(&bytes)).unwrap();
        assert_eq!(candidate.mappings, source.mappings);
        assert_eq!(candidate.registers(), &[7; 128]);
        assert_eq!(candidate.runtime(), source.runtime());
        let mut wrong = Snapshot::candidate(0, &options).unwrap();
        assert_eq!(
            wrong.adopt_record(0, Some(&bytes)),
            Err(Error::UnsupportedVersion)
        );
        let mut wrong_options = options.clone();
        wrong_options.arguments.push("different".into());
        let mut wrong = Snapshot::candidate(1, &wrong_options).unwrap();
        assert_eq!(wrong.adopt_record(0, Some(&bytes)), Err(Error::InvalidData));
    }

    #[test]
    fn checkpoint_rejects_write_execute_mappings() {
        let options = NixRunnerOptions {
            path: "/pkg/bin/looping".into(),
            arguments: vec!["looping".into()],
            environment: Vec::new(),
            rootfs: Default::default(),
        };
        for rights in [4 | 8, 2 | 4 | 8] {
            assert!(matches!(
                Snapshot::source(
                    1,
                    &options,
                    &[7; 128],
                    RuntimeState {
                        signals: vec![1],
                        dispatcher: vec![2],
                        vfs: VfsSnapshot {
                            cwd: "/".into(),
                            descriptors: Vec::new(),
                        },
                        signal_frames: Vec::new(),
                    }
                    .encode()
                    .unwrap(),
                    vec![Mapping {
                        handle: 9,
                        address: 0x10_0000_0000,
                        size: 4096,
                        offset: 0,
                        rights,
                        owned: false,
                    }],
                    Some(Channel(11)),
                ),
                Err(Error::InvalidData)
            ));
        }
    }
}
