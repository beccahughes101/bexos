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
    pub readonly: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NixResourceLimit {
    pub resource: u32,
    pub soft: u64,
    pub hard: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NixRunnerOptions {
    pub path: String,
    pub arguments: Vec<String>,
    pub environment: Vec<Environment>,
    pub rootfs: NixRootFilesystem,
    pub working_directory: String,
    pub uid: u32,
    pub gid: u32,
    pub umask: u32,
    pub resource_limits: Vec<NixResourceLimit>,
    pub hostname: String,
}

impl NixRunnerOptions {
    pub fn validate(&self) -> Result<(), Error> {
        let relative = self.path.strip_prefix('/').ok_or(Error::InvalidOptions)?;
        if self.path.len() > 1024
            || self.path.contains('\0')
            || relative
                .split('/')
                .any(|component| component.is_empty() || matches!(component, "." | ".."))
            || self.arguments.len() > 64
            || self.environment.len() > 64
            || self.resource_limits.len() > 32
            || self.umask & !0o777 != 0
            || self.hostname.len() > 64
            || self.hostname.contains(['/', '\0'])
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
        if !self.working_directory.is_empty() {
            let relative = self
                .working_directory
                .strip_prefix('/')
                .ok_or(Error::InvalidOptions)?;
            if self.working_directory.len() > 4096
                || self.working_directory.contains('\0')
                || relative
                    .split('/')
                    .any(|component| matches!(component, "." | ".."))
            {
                return Err(Error::InvalidOptions);
            }
        }
        for (index, limit) in self.resource_limits.iter().enumerate() {
            if limit.resource > 15
                || limit.soft > limit.hard
                || self.resource_limits[..index]
                    .iter()
                    .any(|prior| prior.resource == limit.resource)
            {
                return Err(Error::InvalidOptions);
            }
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
                            3 => rootfs.readonly = encoded.number()? != 0,
                            _ => {}
                        }
                    }
                    value.rootfs = rootfs;
                }
                5 => value.working_directory = encoded.string()?,
                6 => {
                    value.uid =
                        u32::try_from(encoded.number()?).map_err(|_| Error::InvalidOptions)?
                }
                7 => {
                    value.gid =
                        u32::try_from(encoded.number()?).map_err(|_| Error::InvalidOptions)?
                }
                8 => {
                    value.umask =
                        u32::try_from(encoded.number()?).map_err(|_| Error::InvalidOptions)?
                }
                9 => {
                    if value.resource_limits.len() == 32 {
                        return Err(Error::LimitExceeded);
                    }
                    let mut limit = NixResourceLimit::default();
                    let mut nested = wire::Reader(encoded.bytes()?);
                    while let Some((field, encoded)) = nested.field()? {
                        match field {
                            1 => {
                                limit.resource = u32::try_from(encoded.number()?)
                                    .map_err(|_| Error::InvalidOptions)?
                            }
                            2 => limit.soft = encoded.number()?,
                            3 => limit.hard = encoded.number()?,
                            _ => {}
                        }
                    }
                    value.resource_limits.push(limit);
                }
                10 => value.hostname = encoded.string()?,
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
            if self.rootfs.readonly {
                wire::number(&mut nested, 3, 1);
            }
            wire::bytes(&mut out, 4, &nested);
        }
        if !self.working_directory.is_empty() {
            wire::bytes(&mut out, 5, self.working_directory.as_bytes());
        }
        if self.uid != 0 {
            wire::number(&mut out, 6, u64::from(self.uid));
        }
        if self.gid != 0 {
            wire::number(&mut out, 7, u64::from(self.gid));
        }
        if self.umask != 0 {
            wire::number(&mut out, 8, u64::from(self.umask));
        }
        for limit in &self.resource_limits {
            let mut nested = Vec::new();
            wire::number(&mut nested, 1, u64::from(limit.resource));
            wire::number(&mut nested, 2, limit.soft);
            wire::number(&mut nested, 3, limit.hard);
            wire::bytes(&mut out, 9, &nested);
        }
        if !self.hostname.is_empty() {
            wire::bytes(&mut out, 10, self.hostname.as_bytes());
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn extended_options_round_trip() {
        let options = NixRunnerOptions {
            path: "/bin/tool".into(),
            arguments: vec!["tool".into()],
            working_directory: "/work".into(),
            uid: 1000,
            gid: 100,
            umask: 0o027,
            hostname: "sandbox".into(),
            resource_limits: vec![NixResourceLimit {
                resource: 7,
                soft: 64,
                hard: 128,
            }],
            ..Default::default()
        };
        assert_eq!(
            NixRunnerOptions::decode(&options.encode().unwrap()),
            Ok(options)
        );
    }

    #[test]
    fn invalid_limits_and_paths_are_rejected() {
        let mut options = NixRunnerOptions {
            path: "/bin/tool".into(),
            working_directory: "../escape".into(),
            ..Default::default()
        };
        assert_eq!(options.validate(), Err(Error::InvalidOptions));
        options.working_directory = "/".into();
        options.resource_limits.push(NixResourceLimit {
            resource: 7,
            soft: 2,
            hard: 1,
        });
        assert_eq!(options.validate(), Err(Error::InvalidOptions));
    }
}
