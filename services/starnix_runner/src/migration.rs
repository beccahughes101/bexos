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

const RECORD_VERSION: u64 = 1;
const MAX_MAPPINGS: usize = 96;
const MAX_REGISTER_BYTES: usize = 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Mapping {
    pub handle: u64,
    pub address: u64,
    pub size: u64,
    pub rights: u32,
}

pub(crate) struct Snapshot {
    architecture: u64,
    options: Vec<u8>,
    registers: Vec<u8>,
    pub mappings: Vec<Mapping>,
    migration: Option<Channel>,
    valid: bool,
}

impl Snapshot {
    pub fn source(
        architecture: u64,
        options: &NixRunnerOptions,
        registers: &[u8],
        mappings: Vec<Mapping>,
        migration: Option<Channel>,
    ) -> Result<Self, Error> {
        let value = Self {
            architecture,
            options: options.encode().map_err(|_| Error::InvalidData)?,
            registers: registers.to_vec(),
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
        out.word(self.migration.map_or(0, |channel| channel.0));
        out.word(self.mappings.len() as u64);
        for mapping in &self.mappings {
            out.word(mapping.handle);
            out.word(mapping.address);
            out.word(mapping.size);
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
        let migration = input.word()?;
        self.migration = (migration != 0).then_some(Channel(migration));
        self.mappings.clear();
        for _ in 0..input.count(MAX_MAPPINGS)? {
            self.mappings.push(Mapping {
                handle: input.word()?,
                address: input.word()?,
                size: input.word()?,
                rights: u32::try_from(input.word()?).map_err(|_| Error::InvalidData)?,
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
            || self.mappings.is_empty()
            || self.mappings.len() > MAX_MAPPINGS
            || self.mappings.iter().any(|mapping| {
                mapping.handle == 0
                    || mapping.size == 0
                    || mapping.address & 4095 != 0
                    || mapping.size & 4095 != 0
                    || mapping.rights & !0xe != 0
                    || mapping.rights & 2 == 0
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
                offset: 0,
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
        };
        let source = Snapshot::source(
            1,
            &options,
            &[7; 128],
            vec![Mapping {
                handle: 9,
                address: 0x10_0000_0000,
                size: 4096,
                rights: 2 | 8,
            }],
            Some(Channel(11)),
        )
        .unwrap();
        let bytes = source.encode_record(0).unwrap().unwrap();
        let mut candidate = Snapshot::candidate(1, &options).unwrap();
        candidate.adopt_record(0, Some(&bytes)).unwrap();
        assert_eq!(candidate.mappings, source.mappings);
        assert_eq!(candidate.registers(), &[7; 128]);
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
    fn checkpoint_rejects_write_execute_and_unreadable_mappings() {
        let options = NixRunnerOptions {
            path: "/pkg/bin/looping".into(),
            arguments: vec!["looping".into()],
            environment: Vec::new(),
        };
        for rights in [4, 2 | 4 | 8] {
            assert!(matches!(
                Snapshot::source(
                    1,
                    &options,
                    &[7; 128],
                    vec![Mapping {
                        handle: 9,
                        address: 0x10_0000_0000,
                        size: 4096,
                        rights,
                    }],
                    Some(Channel(11)),
                ),
                Err(Error::InvalidData)
            ));
        }
    }
}
