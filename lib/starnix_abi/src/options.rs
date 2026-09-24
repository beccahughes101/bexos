use crate::{Error, wire};
use alloc::{string::String, vec::Vec};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Environment {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NixRootSource {
    #[default]
    Package,
    Data,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NixRootFilesystem {
    pub source: NixRootSource,
    pub subpath: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NixRunnerOptions {
    pub path: String,
    pub arguments: Vec<String>,
    pub environment: Vec<Environment>,
    pub rootfs: NixRootFilesystem,
}

impl NixRunnerOptions {
    pub fn validate(&self) -> Result<(), Error> {
        let relative = self
            .path
            .strip_prefix("/pkg/")
            .ok_or(Error::InvalidOptions)?;
        if self.path.len() > 1024
            || self.path.contains('\0')
            || relative
                .split('/')
                .any(|component| component.is_empty() || matches!(component, "." | ".."))
            || self.arguments.len() > 64
            || self.environment.len() > 64
        {
            return Err(Error::InvalidOptions);
        }
        if self
            .arguments
            .iter()
            .any(|argument| argument.len() > 4096 || argument.contains('\0'))
        {
            return Err(Error::InvalidOptions);
        }
        for (index, environment) in self.environment.iter().enumerate() {
            if environment.name.is_empty()
                || environment.name.len() > 256
                || environment.name.contains(['=', '\0'])
                || environment.value.len() > 4096
                || environment.value.contains('\0')
                || self.environment[..index]
                    .iter()
                    .any(|prior| prior.name == environment.name)
            {
                return Err(Error::InvalidOptions);
            }
        }
        if self.rootfs.subpath.len() > 255 || self.rootfs.subpath.contains('\0') {
            return Err(Error::InvalidOptions);
        }
        let root = self.rootfs.subpath.trim_matches('/');
        if !root.is_empty()
            && root
                .split('/')
                .any(|component| component.is_empty() || matches!(component, "." | ".."))
        {
            return Err(Error::InvalidOptions);
        }
        Ok(())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > 32_768 {
            return Err(Error::LimitExceeded);
        }
        let mut value = Self::default();
        let mut reader = wire::Reader(bytes);
        while let Some((field, encoded)) = reader.field()? {
            match field {
                1 => value.path = encoded.string()?,
                2 => {
                    if value.arguments.len() == 64 {
                        return Err(Error::LimitExceeded);
                    }
                    value.arguments.push(encoded.string()?);
                }
                3 => {
                    if value.environment.len() == 64 {
                        return Err(Error::LimitExceeded);
                    }
                    let mut environment = Environment::default();
                    let mut nested = wire::Reader(encoded.bytes()?);
                    while let Some((field, encoded)) = nested.field()? {
                        match field {
                            1 => environment.name = encoded.string()?,
                            2 => environment.value = encoded.string()?,
                            _ => {}
                        }
                    }
                    value.environment.push(environment);
                }
                4 => {
                    let mut rootfs = NixRootFilesystem::default();
                    let mut nested = wire::Reader(encoded.bytes()?);
                    while let Some((field, encoded)) = nested.field()? {
                        match field {
                            1 => {
                                rootfs.source = match encoded.number()? {
                                    0 => NixRootSource::Package,
                                    1 => NixRootSource::Data,
                                    _ => return Err(Error::InvalidOptions),
                                }
                            }
                            2 => rootfs.subpath = encoded.string()?,
                            _ => {}
                        }
                    }
                    value.rootfs = rootfs;
                }
                _ => {}
            }
        }
        value.validate()?;
        Ok(value)
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        let mut out = Vec::new();
        wire::bytes(&mut out, 1, self.path.as_bytes());
        for argument in &self.arguments {
            wire::bytes(&mut out, 2, argument.as_bytes());
        }
        for environment in &self.environment {
            let mut nested = Vec::new();
            wire::bytes(&mut nested, 1, environment.name.as_bytes());
            wire::bytes(&mut nested, 2, environment.value.as_bytes());
            wire::bytes(&mut out, 3, &nested);
        }
        if self.rootfs != NixRootFilesystem::default() {
            let mut nested = Vec::new();
            wire::number(
                &mut nested,
                1,
                match self.rootfs.source {
                    NixRootSource::Package => 0,
                    NixRootSource::Data => 1,
                },
            );
            if !self.rootfs.subpath.is_empty() {
                wire::bytes(&mut nested, 2, self.rootfs.subpath.as_bytes());
            }
            wire::bytes(&mut out, 4, &nested);
        }
        Ok(out)
    }
}
