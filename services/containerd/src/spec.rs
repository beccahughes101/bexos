use std::fmt::Write;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ImageReference {
    pub registry_host: String,
    pub repository: String,
    pub tag: String,
    pub expected_sha256: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Resources {
    pub cpu_shares: u32,
    pub memory_limit_bytes: u64,
    pub process_limit: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ContainerSpec {
    pub version: u32,
    pub container_id: String,
    pub image: ImageReference,
    pub arguments: Vec<String>,
    pub environment: Vec<String>,
    pub working_directory: String,
    pub uid: u32,
    pub gid: u32,
    pub hostname: String,
    pub resources: Resources,
    pub readonly_rootfs: bool,
    pub manifest_digest: [u8; 32],
}

impl ContainerSpec {
    pub fn validate(&self) -> Result<(), ()> {
        if self.version != 1
            || !valid_id(&self.container_id)
            || self.image.registry_host.is_empty()
            || self.image.registry_host.len() > 128
            || self.image.repository.is_empty()
            || self.image.repository.len() > 128
            || self.image.tag.is_empty()
            || self.image.tag.len() > 64
            || !matches!(self.image.expected_sha256.len(), 0 | 32)
            || self.arguments.is_empty()
            || self.arguments.len() > 64
            || self.environment.len() > 64
            || self.working_directory.len() > 4096
            || !self.working_directory.starts_with('/')
            || self.hostname.len() > 64
            || self
                .arguments
                .iter()
                .any(|s| s.len() > 4096 || s.contains('\0'))
            || self.environment.iter().any(|s| {
                s.len() > 4096
                    || s.contains('\0')
                    || s.split_once('=').is_none_or(|(name, _)| name.is_empty())
            })
        {
            return Err(());
        }
        Ok(())
    }

    pub fn encode_prototxt(&self) -> Result<String, ()> {
        self.validate()?;
        let mut out = String::new();
        writeln!(out, "version: {}", self.version).unwrap();
        string_field(&mut out, "container_id", &self.container_id);
        out.push_str("image {\n");
        string_field_indented(&mut out, "registry_host", &self.image.registry_host);
        string_field_indented(&mut out, "repository", &self.image.repository);
        string_field_indented(&mut out, "tag", &self.image.tag);
        bytes_field_indented(&mut out, "expected_sha256", &self.image.expected_sha256);
        out.push_str("}\n");
        for value in &self.arguments {
            string_field(&mut out, "arguments", value);
        }
        for value in &self.environment {
            string_field(&mut out, "environment", value);
        }
        string_field(&mut out, "working_directory", &self.working_directory);
        writeln!(out, "uid: {}", self.uid).unwrap();
        writeln!(out, "gid: {}", self.gid).unwrap();
        string_field(&mut out, "hostname", &self.hostname);
        out.push_str("resources {\n");
        writeln!(out, "  cpu_shares: {}", self.resources.cpu_shares).unwrap();
        writeln!(
            out,
            "  memory_limit_bytes: {}",
            self.resources.memory_limit_bytes
        )
        .unwrap();
        writeln!(out, "  process_limit: {}", self.resources.process_limit).unwrap();
        out.push_str("}\n");
        writeln!(out, "readonly_rootfs: {}", self.readonly_rootfs).unwrap();
        bytes_field(&mut out, "manifest_digest", &self.manifest_digest);
        Ok(out)
    }

    pub fn decode_prototxt(bytes: &[u8]) -> Result<Self, ()> {
        if bytes.len() > 64 * 1024 {
            return Err(());
        }
        let text = core::str::from_utf8(bytes).map_err(|_| ())?;
        let mut parser = Parser::new(text)?;
        let mut spec = Self::default();
        while !parser.done() {
            let field = parser.ident()?;
            match field {
                "version" => spec.version = parser.number_u32()?,
                "container_id" => spec.container_id = parser.string()?,
                "arguments" => spec.arguments.push(parser.string()?),
                "environment" => spec.environment.push(parser.string()?),
                "working_directory" => spec.working_directory = parser.string()?,
                "uid" => spec.uid = parser.number_u32()?,
                "gid" => spec.gid = parser.number_u32()?,
                "hostname" => spec.hostname = parser.string()?,
                "readonly_rootfs" => spec.readonly_rootfs = parser.boolean()?,
                "manifest_digest" => {
                    let digest = parser.bytes()?;
                    spec.manifest_digest = digest.try_into().map_err(|_| ())?;
                }
                "image" => parser.message(|p| {
                    while !p.at_close() {
                        match p.ident()? {
                            "registry_host" => spec.image.registry_host = p.string()?,
                            "repository" => spec.image.repository = p.string()?,
                            "tag" => spec.image.tag = p.string()?,
                            "expected_sha256" => spec.image.expected_sha256 = p.bytes()?,
                            _ => return Err(()),
                        }
                    }
                    Ok(())
                })?,
                "resources" => parser.message(|p| {
                    while !p.at_close() {
                        match p.ident()? {
                            "cpu_shares" => spec.resources.cpu_shares = p.number_u32()?,
                            "memory_limit_bytes" => {
                                spec.resources.memory_limit_bytes = p.number_u64()?
                            }
                            "process_limit" => spec.resources.process_limit = p.number_u32()?,
                            _ => return Err(()),
                        }
                    }
                    Ok(())
                })?,
                _ => return Err(()),
            }
        }
        spec.validate()?;
        Ok(spec)
    }
}

pub fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

fn escaped(value: &[u8]) -> String {
    let mut result = String::new();
    for byte in value {
        match *byte {
            b'\\' => result.push_str("\\\\"),
            b'\"' => result.push_str("\\\""),
            b'\n' => result.push_str("\\n"),
            b'\r' => result.push_str("\\r"),
            b'\t' => result.push_str("\\t"),
            0x20..=0x7e => result.push(*byte as char),
            value => write!(result, "\\{:03o}", value).unwrap(),
        }
    }
    result
}
fn string_field(out: &mut String, name: &str, value: &str) {
    writeln!(out, "{name}: \"{}\"", escaped(value.as_bytes())).unwrap();
}
fn string_field_indented(out: &mut String, name: &str, value: &str) {
    writeln!(out, "  {name}: \"{}\"", escaped(value.as_bytes())).unwrap();
}
fn bytes_field(out: &mut String, name: &str, value: &[u8]) {
    writeln!(out, "{name}: \"{}\"", escaped(value)).unwrap();
}
fn bytes_field_indented(out: &mut String, name: &str, value: &[u8]) {
    writeln!(out, "  {name}: \"{}\"", escaped(value)).unwrap();
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Result<Self, ()> {
        Ok(Self {
            bytes: text.as_bytes(),
            at: 0,
        })
    }
    fn whitespace(&mut self) {
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }
    fn done(&mut self) -> bool {
        self.whitespace();
        self.at == self.bytes.len()
    }
    fn at_close(&mut self) -> bool {
        self.whitespace();
        self.bytes.get(self.at) == Some(&b'}')
    }
    fn take(&mut self, value: u8) -> Result<(), ()> {
        self.whitespace();
        if self.bytes.get(self.at) != Some(&value) {
            return Err(());
        }
        self.at += 1;
        Ok(())
    }
    fn ident(&mut self) -> Result<&'a str, ()> {
        self.whitespace();
        let start = self.at;
        while self
            .bytes
            .get(self.at)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        {
            self.at += 1;
        }
        if self.at == start {
            return Err(());
        }
        core::str::from_utf8(&self.bytes[start..self.at]).map_err(|_| ())
    }
    fn scalar(&mut self) -> Result<&'a str, ()> {
        self.take(b':')?;
        self.whitespace();
        let start = self.at;
        while self
            .bytes
            .get(self.at)
            .is_some_and(|b| b.is_ascii_alphanumeric())
        {
            self.at += 1;
        }
        if self.at == start {
            return Err(());
        }
        core::str::from_utf8(&self.bytes[start..self.at]).map_err(|_| ())
    }
    fn number_u64(&mut self) -> Result<u64, ()> {
        self.scalar()?.parse().map_err(|_| ())
    }
    fn number_u32(&mut self) -> Result<u32, ()> {
        self.scalar()?.parse().map_err(|_| ())
    }
    fn boolean(&mut self) -> Result<bool, ()> {
        match self.scalar()? {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(()),
        }
    }
    fn bytes(&mut self) -> Result<Vec<u8>, ()> {
        self.take(b':')?;
        self.take(b'\"')?;
        let mut out = Vec::new();
        loop {
            let byte = *self.bytes.get(self.at).ok_or(())?;
            self.at += 1;
            match byte {
                b'\"' => break,
                b'\\' => {
                    let next = *self.bytes.get(self.at).ok_or(())?;
                    self.at += 1;
                    match next {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'\\' | b'\"' => out.push(next),
                        b'0'..=b'7' => {
                            let b = *self.bytes.get(self.at).ok_or(())?;
                            let c = *self.bytes.get(self.at + 1).ok_or(())?;
                            if !(b'0'..=b'7').contains(&b) || !(b'0'..=b'7').contains(&c) {
                                return Err(());
                            }
                            self.at += 2;
                            let value = u16::from(next - b'0') * 64
                                + u16::from(b - b'0') * 8
                                + u16::from(c - b'0');
                            out.push(value.try_into().map_err(|_| ())?);
                        }
                        _ => return Err(()),
                    }
                }
                0..=0x1f => return Err(()),
                value => out.push(value),
            }
        }
        Ok(out)
    }
    fn string(&mut self) -> Result<String, ()> {
        String::from_utf8(self.bytes()?).map_err(|_| ())
    }
    fn message(&mut self, f: impl FnOnce(&mut Self) -> Result<(), ()>) -> Result<(), ()> {
        self.take(b'{')?;
        f(self)?;
        self.take(b'}')
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prototxt_round_trip_preserves_binary_digest_and_escapes() {
        let spec = ContainerSpec {
            version: 1,
            container_id: "demo-1".into(),
            image: ImageReference {
                registry_host: "registry.test".into(),
                repository: "base/demo".into(),
                tag: "v1".into(),
                expected_sha256: vec![7; 32],
            },
            arguments: vec!["/bin/echo".into(), "a\n\"b".into()],
            environment: vec!["PATH=/bin".into()],
            working_directory: "/".into(),
            uid: 12,
            gid: 34,
            hostname: "demo".into(),
            resources: Resources {
                cpu_shares: 20,
                memory_limit_bytes: 4096,
                process_limit: 8,
            },
            readonly_rootfs: true,
            manifest_digest: [0xff; 32],
        };
        let encoded = spec.encode_prototxt().unwrap();
        assert_eq!(
            ContainerSpec::decode_prototxt(encoded.as_bytes()).unwrap(),
            spec
        );
    }
    #[test]
    fn rejects_traversal_ids_and_unknown_fields() {
        assert!(!valid_id(".."));
        assert!(ContainerSpec::decode_prototxt(b"version: 1\nunknown: 2\n").is_err());
    }

    #[test]
    fn round_trip_allows_comment_markers_inside_strings_only() {
        let mut spec = ContainerSpec {
            version: 1,
            container_id: "demo".into(),
            image: ImageReference {
                registry_host: "registry.test".into(),
                repository: "team/image".into(),
                tag: "v1".into(),
                expected_sha256: vec![],
            },
            arguments: vec!["/bin/sh".into(), "https://example.invalid/#fragment".into()],
            environment: vec![],
            working_directory: "/".into(),
            hostname: "demo".into(),
            manifest_digest: [1; 32],
            ..ContainerSpec::default()
        };
        let encoded = spec.encode_prototxt().unwrap();
        assert_eq!(
            ContainerSpec::decode_prototxt(encoded.as_bytes()).unwrap(),
            spec
        );
        spec.arguments[1] = "# still data".into();
        let encoded = spec.encode_prototxt().unwrap();
        assert_eq!(
            ContainerSpec::decode_prototxt(encoded.as_bytes()).unwrap(),
            spec
        );
        assert!(ContainerSpec::decode_prototxt(b"# outside a string\n").is_err());
    }
}
