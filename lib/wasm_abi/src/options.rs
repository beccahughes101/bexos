use crate::{
    Error,
    wire::{self, Reader},
};
use alloc::{string::String, vec::Vec};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Limits {
    pub max_module_bytes: u64,
    pub max_memory_pages: u64,
    pub max_table_elements: u64,
    pub max_stack_bytes: u64,
    pub max_handles: u64,
    pub max_children: u64,
    pub fuel: u64,
    pub fuel_slice: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_module_bytes: 16 << 20,
            max_memory_pages: 1024,
            max_table_elements: 65536,
            max_stack_bytes: 512 << 10,
            max_handles: 256,
            max_children: 8,
            fuel: 100_000_000,
            fuel_slice: 10_000,
        }
    }
}
impl Limits {
    pub fn validate(&self) -> Result<(), Error> {
        if self.max_module_bytes == 0
            || self.max_module_bytes > 64 << 20
            || self.max_memory_pages == 0
            || self.max_memory_pages > 4096
            || self.max_table_elements == 0
            || self.max_table_elements > 1_000_000
            || self.max_stack_bytes < 65536
            || self.max_stack_bytes > 2 << 20
            || self.max_handles == 0
            || self.max_handles > 4096
            || self.max_children > 64
            || self.fuel == 0
            || self.fuel > 10_000_000_000
            || self.fuel_slice == 0
            || self.fuel_slice > self.fuel
            || self.fuel_slice > 100_000
        {
            return Err(Error::LimitExceeded);
        }
        Ok(())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut result = Self::default();
        let mut r = Reader(bytes);
        while let Some((key, value)) = r.field()? {
            let field = match key {
                1 => &mut result.max_module_bytes,
                2 => &mut result.max_memory_pages,
                3 => &mut result.max_table_elements,
                4 => &mut result.max_stack_bytes,
                5 => &mut result.max_handles,
                6 => &mut result.max_children,
                7 => &mut result.fuel,
                8 => &mut result.fuel_slice,
                _ => continue,
            };
            *field = value.number()?;
        }
        result.validate()?;
        Ok(result)
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, n) in [
            self.max_module_bytes,
            self.max_memory_pages,
            self.max_table_elements,
            self.max_stack_bytes,
            self.max_handles,
            self.max_children,
            self.fuel,
            self.fuel_slice,
        ]
        .iter()
        .enumerate()
        {
            wire::number(&mut out, i as u32 + 1, *n);
        }
        out
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Environment {
    pub name: String,
    pub value: String,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComponentImport {
    pub package_name: String,
    pub export_name: String,
    pub abi_version: u32,
    pub instance_name: String,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WasmRunnerOptions {
    pub path: String,
    pub arguments: Vec<String>,
    pub environment: Vec<Environment>,
    pub limits: Limits,
    pub component_imports: Vec<ComponentImport>,
}
impl WasmRunnerOptions {
    pub fn validate(&self) -> Result<(), Error> {
        let path = self
            .path
            .strip_prefix("/pkg/")
            .ok_or(Error::InvalidOptions)?;
        if self.path.len() > 1024
            || self.path.contains('\0')
            || path
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == "..")
        {
            return Err(Error::InvalidOptions);
        }
        if self.arguments.len() > 64 || self.environment.len() > 64 {
            return Err(Error::LimitExceeded);
        }
        for arg in &self.arguments {
            if arg.len() > 4096 || arg.contains('\0') {
                return Err(Error::InvalidOptions);
            }
        }
        for (i, e) in self.environment.iter().enumerate() {
            if e.name.is_empty()
                || e.name.len() > 256
                || e.name.contains(['=', '\0'])
                || e.value.len() > 4096
                || e.value.contains('\0')
                || self.environment[..i].iter().any(|old| old.name == e.name)
            {
                return Err(Error::InvalidOptions);
            }
        }
        if self.component_imports.len() > 16 {
            return Err(Error::LimitExceeded);
        }
        for (i, import) in self.component_imports.iter().enumerate() {
            if import.package_name.is_empty()
                || import.package_name.len() > 128
                || import.package_name.contains(['/', '\0'])
                || import.export_name.is_empty()
                || import.export_name.len() > 128
                || import.export_name.contains('\0')
                || import.abi_version == 0
                || import.instance_name.is_empty()
                || import.instance_name.len() > 128
                || import.instance_name.contains('\0')
                || self.component_imports[..i]
                    .iter()
                    .any(|old| old.instance_name == import.instance_name)
            {
                return Err(Error::InvalidOptions);
            }
        }
        self.limits.validate()
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > 32768 {
            return Err(Error::LimitExceeded);
        }
        let mut r = Reader(bytes);
        let mut s = Self::default();
        while let Some((key, v)) = r.field()? {
            match key {
                1 => s.path = v.string()?,
                2 => {
                    if s.arguments.len() == 64 {
                        return Err(Error::LimitExceeded);
                    }
                    s.arguments.push(v.string()?);
                }
                3 => {
                    if s.environment.len() == 64 {
                        return Err(Error::LimitExceeded);
                    }
                    let mut e = Environment::default();
                    let mut r = Reader(v.bytes()?);
                    while let Some((key, v)) = r.field()? {
                        match key {
                            1 => e.name = v.string()?,
                            2 => e.value = v.string()?,
                            _ => {}
                        }
                    }
                    s.environment.push(e);
                }
                4 => s.limits = Limits::decode(v.bytes()?)?,
                5 => {
                    if s.component_imports.len() == 16 {
                        return Err(Error::LimitExceeded);
                    }
                    let mut import = ComponentImport::default();
                    let mut r = Reader(v.bytes()?);
                    while let Some((key, v)) = r.field()? {
                        match key {
                            1 => import.package_name = v.string()?,
                            2 => import.export_name = v.string()?,
                            3 => import.abi_version = v.number()? as u32,
                            4 => import.instance_name = v.string()?,
                            _ => {}
                        }
                    }
                    s.component_imports.push(import);
                }
                _ => {}
            }
        }
        s.validate()?;
        Ok(s)
    }
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        let mut out = Vec::new();
        wire::bytes(&mut out, 1, self.path.as_bytes());
        for arg in &self.arguments {
            wire::bytes(&mut out, 2, arg.as_bytes());
        }
        for e in &self.environment {
            let mut bytes = Vec::new();
            wire::bytes(&mut bytes, 1, e.name.as_bytes());
            wire::bytes(&mut bytes, 2, e.value.as_bytes());
            wire::bytes(&mut out, 3, &bytes);
        }
        wire::bytes(&mut out, 4, &self.limits.encode());
        for import in &self.component_imports {
            let mut bytes = Vec::new();
            wire::bytes(&mut bytes, 1, import.package_name.as_bytes());
            wire::bytes(&mut bytes, 2, import.export_name.as_bytes());
            wire::number(&mut bytes, 3, import.abi_version as u64);
            wire::bytes(&mut bytes, 4, import.instance_name.as_bytes());
            wire::bytes(&mut out, 5, &bytes);
        }
        if out.len() > 32768 {
            return Err(Error::LimitExceeded);
        }
        Ok(out)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn options_roundtrip_and_validation() {
        let mut o = WasmRunnerOptions {
            path: "/pkg/app.wasm".into(),
            ..Default::default()
        };
        o.component_imports.push(ComponentImport {
            package_name: "com.bexos.lib.dioxus".into(),
            export_name: "dioxus".into(),
            abi_version: 1,
            instance_name: "com:bexos/lib/dioxus@1.0.0".into(),
        });
        assert_eq!(WasmRunnerOptions::decode(&o.encode().unwrap()).unwrap(), o);
        for p in [
            "/pkg/../app.wasm",
            "/pkg//app.wasm",
            "/data/app.wasm",
            "/pkg/./a",
            "/pkg/",
        ] {
            o.path = p.into();
            assert!(o.encode().is_err());
        }
    }
    #[test]
    fn malformed_and_unbounded_options() {
        assert!(WasmRunnerOptions::decode(&[0xff; 12]).is_err());
        assert!(Limits::decode(&[0x38, 0]).is_err());
        let mut l = Limits::default();
        l.fuel_slice = l.fuel + 1;
        assert!(l.validate().is_err());
    }
}
