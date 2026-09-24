use crate::{ABI_VERSION, Error, MAX_IMAGE_BYTES, NixRunnerOptions, STARTUP_MAGIC, wire};
use alloc::vec::Vec;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Launch {
    pub options: NixRunnerOptions,
    pub image_len: u64,
    pub service: bool,
    pub migratable: bool,
}

impl Launch {
    pub fn validate(&self) -> Result<(), Error> {
        self.options.validate()?;
        if self.image_len < 64
            || self.image_len > MAX_IMAGE_BYTES
            || (self.service && !self.migratable)
        {
            return Err(Error::InvalidOptions);
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        let mut out = STARTUP_MAGIC.to_vec();
        wire::number(&mut out, 1, u64::from(ABI_VERSION));
        wire::bytes(&mut out, 2, &self.options.encode()?);
        wire::number(&mut out, 3, self.image_len);
        wire::number(&mut out, 4, self.service as u64);
        wire::number(&mut out, 5, self.migratable as u64);
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = wire::Reader(
            bytes
                .strip_prefix(STARTUP_MAGIC)
                .ok_or(Error::InvalidEncoding)?,
        );
        let (mut version, mut options, mut image_len, mut service, mut migratable) =
            (0, None, 0, false, false);
        let mut required = 0u8;
        while let Some((field, value)) = reader.field()? {
            match field {
                1 => {
                    version = value.number()?;
                    required |= 1 << 0;
                }
                2 => {
                    options = Some(NixRunnerOptions::decode(value.bytes()?)?);
                    required |= 1 << 1;
                }
                3 => {
                    image_len = value.number()?;
                    required |= 1 << 2;
                }
                4 => {
                    service = value.number()? != 0;
                    required |= 1 << 3;
                }
                5 => {
                    migratable = value.number()? != 0;
                    required |= 1 << 4;
                }
                _ => {}
            }
        }
        if required != 0b1_1111 || version != u64::from(ABI_VERSION) {
            return Err(Error::InvalidEncoding);
        }
        let value = Self {
            options: options.ok_or(Error::InvalidOptions)?,
            image_len,
            service,
            migratable,
        };
        value.validate()?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Environment;
    use alloc::{string::ToString, vec};

    fn launch() -> Launch {
        Launch {
            options: NixRunnerOptions {
                path: "/pkg/bin/hello".to_string(),
                arguments: vec!["hello".to_string()],
                environment: vec![Environment {
                    name: "LANG".to_string(),
                    value: "C".to_string(),
                }],
            },
            image_len: 4096,
            service: true,
            migratable: true,
        }
    }

    #[test]
    fn launch_round_trips() {
        let value = launch();
        assert_eq!(Launch::decode(&value.encode().unwrap()).unwrap(), value);
    }

    #[test]
    fn options_reject_parent_path_duplicate_environment_and_non_migratable_service() {
        let mut value = launch();
        value.options.path = "/pkg/../escape".to_string();
        assert_eq!(value.encode(), Err(Error::InvalidOptions));

        let mut value = launch();
        value
            .options
            .environment
            .push(value.options.environment[0].clone());
        assert_eq!(value.encode(), Err(Error::InvalidOptions));

        let mut value = launch();
        value.migratable = false;
        assert_eq!(value.encode(), Err(Error::InvalidOptions));
    }

    #[test]
    fn decoder_rejects_truncation_and_wrong_magic() {
        let encoded = launch().encode().unwrap();
        for index in 0..encoded.len() {
            assert!(Launch::decode(&encoded[..index]).is_err());
        }
        let mut wrong = encoded;
        wrong[0] ^= 1;
        assert_eq!(Launch::decode(&wrong), Err(Error::InvalidEncoding));
    }
}
