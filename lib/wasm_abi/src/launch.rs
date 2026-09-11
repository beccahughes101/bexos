use crate::{
    ComponentImport, Error, WasmRunnerOptions,
    wire::{self, Reader},
};
use alloc::{string::String, vec::Vec};
/// Runner-only prelude preceding the unchanged userspace Startup message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Launch {
    pub options: WasmRunnerOptions,
    pub module_len: u64,
    pub service: bool,
    pub migratable: bool,
    pub component_dependencies: Vec<ComponentDependency>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentDependency {
    pub import: ComponentImport,
    pub module_len: u64,
}
impl Launch {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        let mut out = crate::STARTUP_MAGIC.to_vec();
        wire::bytes(&mut out, 1, &self.options.encode()?);
        wire::number(&mut out, 2, self.module_len);
        wire::number(&mut out, 3, self.service as u64);
        wire::number(&mut out, 4, self.migratable as u64);
        for dependency in &self.component_dependencies {
            let mut bytes = Vec::new();
            wire::bytes(&mut bytes, 1, dependency.import.package_name.as_bytes());
            wire::bytes(&mut bytes, 2, dependency.import.export_name.as_bytes());
            wire::number(&mut bytes, 3, dependency.import.abi_version as u64);
            wire::bytes(&mut bytes, 4, dependency.import.instance_name.as_bytes());
            wire::number(&mut bytes, 5, dependency.module_len);
            wire::bytes(&mut out, 5, &bytes);
        }
        Ok(out)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader(
            bytes
                .strip_prefix(crate::STARTUP_MAGIC)
                .ok_or(Error::InvalidEncoding)?,
        );
        let (mut options, mut len, mut service, mut migratable) = (None, 0, false, false);
        let mut component_dependencies = Vec::new();
        while let Some((key, v)) = r.field()? {
            match key {
                1 => options = Some(WasmRunnerOptions::decode(v.bytes()?)?),
                2 => len = v.number()?,
                3 => service = v.number()? != 0,
                4 => migratable = v.number()? != 0,
                5 => {
                    if component_dependencies.len() == 16 {
                        return Err(Error::LimitExceeded);
                    }
                    component_dependencies.push(decode_dependency(v.bytes()?)?);
                }
                _ => {}
            }
        }
        let s = Self {
            options: options.ok_or(Error::InvalidOptions)?,
            module_len: len,
            service,
            migratable,
            component_dependencies,
        };
        s.validate()?;
        Ok(s)
    }

    pub fn validate(&self) -> Result<(), Error> {
        self.options.validate()?;
        if self.module_len < 8
            || self.module_len > self.options.limits.max_module_bytes
            || (self.service && !self.migratable)
            || self.component_dependencies.len() != self.options.component_imports.len()
        {
            return Err(Error::InvalidOptions);
        }
        let mut total = self.module_len;
        for (expected, dependency) in self
            .options
            .component_imports
            .iter()
            .zip(self.component_dependencies.iter())
        {
            if expected != &dependency.import
                || dependency.module_len < 8
                || dependency.module_len > self.options.limits.max_module_bytes
            {
                return Err(Error::InvalidOptions);
            }
            total = total
                .checked_add(dependency.module_len)
                .ok_or(Error::LimitExceeded)?;
        }
        if total > self.options.limits.max_module_bytes.saturating_mul(17) {
            return Err(Error::LimitExceeded);
        }
        Ok(())
    }
}

fn decode_dependency(bytes: &[u8]) -> Result<ComponentDependency, Error> {
    let mut r = Reader(bytes);
    let mut import = ComponentImport {
        package_name: String::new(),
        export_name: String::new(),
        abi_version: 0,
        instance_name: String::new(),
    };
    let mut module_len = 0;
    while let Some((key, v)) = r.field()? {
        match key {
            1 => import.package_name = v.string()?,
            2 => import.export_name = v.string()?,
            3 => import.abi_version = v.number()? as u32,
            4 => import.instance_name = v.string()?,
            5 => module_len = v.number()?,
            _ => {}
        }
    }
    Ok(ComponentDependency { import, module_len })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn import() -> ComponentImport {
        ComponentImport {
            package_name: "com.bexos.lib.dioxus".into(),
            export_name: "dioxus".into(),
            abi_version: 1,
            instance_name: "com:bexos/lib/dioxus@1.0.0".into(),
        }
    }

    #[test]
    fn launch_roundtrips_component_dependencies() {
        let mut options = WasmRunnerOptions {
            path: "/pkg/bin/app.wasm".into(),
            ..Default::default()
        };
        options.component_imports.push(import());
        let launch = Launch {
            options,
            module_len: 8,
            service: true,
            migratable: true,
            component_dependencies: vec![ComponentDependency {
                import: import(),
                module_len: 8,
            }],
        };
        assert_eq!(Launch::decode(&launch.encode().unwrap()).unwrap(), launch);
    }

    #[test]
    fn launch_rejects_dependency_identity_mismatch() {
        let mut options = WasmRunnerOptions {
            path: "/pkg/bin/app.wasm".into(),
            ..Default::default()
        };
        options.component_imports.push(import());
        let mut wrong = import();
        wrong.export_name = "native".into();
        let launch = Launch {
            options,
            module_len: 8,
            service: true,
            migratable: true,
            component_dependencies: vec![ComponentDependency {
                import: wrong,
                module_len: 8,
            }],
        };
        assert_eq!(launch.encode(), Err(Error::InvalidOptions));
    }
}
