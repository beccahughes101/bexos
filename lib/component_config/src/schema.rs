use crate::{ConfigTable, ConfigType, encode_config_v2, wire};
use alloc::{
    collections::{BTreeMap, BTreeSet},
    format,
    string::{String, ToString},
    vec::Vec,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Malformed,
    InvalidSchema,
    UnknownField,
    MissingField,
    TypeMismatch,
    Bounds,
    Fingerprint,
    AccessDenied,
    Conflict,
    Busy,
    Storage,
    CommitUncertain,
    Rejected,
    Timeout,
    Overflow,
}
impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum Type {
    #[default]
    Unspecified = 0,
    Bool = 1,
    Uint32 = 2,
    Uint64 = 3,
    String = 4,
    Bytes = 5,
}
impl Type {
    pub fn from_proto(v: u64) -> Self {
        match v {
            1 => Self::Bool,
            2 => Self::Uint32,
            3 => Self::Uint64,
            4 => Self::String,
            5 => Self::Bytes,
            _ => Self::Unspecified,
        }
    }
    pub fn to_wire(self) -> u8 {
        self as u8
    }
    pub fn scalar(self) -> Result<ConfigType, Error> {
        Ok(match self {
            Self::Bool => ConfigType::Bool,
            Self::Uint32 => ConfigType::Uint32,
            Self::Uint64 => ConfigType::Uint64,
            Self::String => ConfigType::String,
            Self::Bytes => ConfigType::Bytes,
            _ => return Err(Error::InvalidSchema),
        })
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum Scope {
    #[default]
    SystemOnly = 0,
    UserEditable = 1,
    MdmLockable = 2,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Constraint {
    UintRange { min: u64, max: u64 },
    StringEnum(Vec<String>),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Value {
    Bool(bool),
    Uint32(u32),
    Uint64(u64),
    String(String),
    Bytes(Vec<u8>),
}
impl Value {
    pub fn config_type(&self) -> Type {
        match self {
            Self::Bool(_) => Type::Bool,
            Self::Uint32(_) => Type::Uint32,
            Self::Uint64(_) => Type::Uint64,
            Self::String(_) => Type::String,
            Self::Bytes(_) => Type::Bytes,
        }
    }
    pub fn bytes(&self) -> Vec<u8> {
        match self {
            Self::Bool(v) => alloc::vec![u8::from(*v)],
            Self::Uint32(v) => v.to_le_bytes().to_vec(),
            Self::Uint64(v) => v.to_le_bytes().to_vec(),
            Self::String(v) => v.as_bytes().to_vec(),
            Self::Bytes(v) => v.clone(),
        }
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut value = None;
        for field in wire::fields(bytes) {
            let (n, k, b) = field?;
            if !(1..=5).contains(&n) {
                continue;
            }
            if value.is_some() {
                return Err(Error::Malformed);
            }
            value = Some(match n {
                1 => Self::Bool(
                    number(k, b)?
                        .try_into()
                        .ok()
                        .filter(|v: &u8| *v <= 1)
                        .ok_or(Error::Malformed)?
                        != 0,
                ),
                2 => Self::Uint32(number(k, b)?.try_into().map_err(|_| Error::Malformed)?),
                3 => Self::Uint64(number(k, b)?),
                4 => Self::String(text(k, b)?),
                5 => {
                    message(k, b)?;
                    Self::Bytes(b.to_vec())
                }
                _ => unreachable!(),
            });
        }
        value.ok_or(Error::Malformed)
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Self::Bool(v) => wire::word(&mut out, 1, u64::from(*v)),
            Self::Uint32(v) => wire::word(&mut out, 2, u64::from(*v)),
            Self::Uint64(v) => wire::word(&mut out, 3, *v),
            Self::String(v) => wire::bytes(&mut out, 4, v.as_bytes()),
            Self::Bytes(v) => wire::bytes(&mut out, 5, v),
        };
        out
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Field {
    pub name: String,
    pub config_type: Type,
    pub required: bool,
    pub default_value: Option<Value>,
    pub max_size: u32,
    pub scope: Scope,
    pub display_name: String,
    pub description: String,
    pub constraint: Option<Constraint>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Schema {
    pub fields: Vec<Field>,
}
pub type Assignments = BTreeMap<String, Value>;

impl Field {
    pub fn validate_value(&self, value: &Value) -> Result<Vec<u8>, String> {
        if self.config_type != value.config_type() {
            return Err(format!("type mismatch for config field {}", self.name));
        }
        let bytes = value.bytes();
        if self.max_size != 0 && bytes.len() > self.max_size as usize {
            return Err(format!("config field {} exceeds max_size", self.name));
        }
        let valid = match (&self.constraint, value) {
            (None, _) => true,
            (Some(Constraint::UintRange { min, max }), Value::Uint32(v)) => {
                (*min..=*max).contains(&u64::from(*v))
            }
            (Some(Constraint::UintRange { min, max }), Value::Uint64(v)) => {
                (*min..=*max).contains(v)
            }
            (Some(Constraint::StringEnum(values)), Value::String(v)) => values.contains(v),
            _ => false,
        };
        if !valid {
            return Err(format!("config field {} violates constraint", self.name));
        }
        Ok(bytes)
    }
}
impl Schema {
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut schema = Self::default();
        for field in wire::fields(bytes) {
            let (n, k, b) = field?;
            if n == 1 {
                schema.fields.push(decode_field(message(k, b)?)?);
            }
        }
        Ok(schema)
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for f in &self.fields {
            wire::bytes(&mut out, 1, &encode_field(f));
        }
        out
    }
    pub fn validate(&self) -> Result<(), String> {
        let mut names = BTreeSet::new();
        for f in &self.fields {
            if f.name.is_empty() || f.name.len() > 64 || f.config_type == Type::Unspecified {
                return Err("config fields require a name and type".into());
            }
            let mut chars = f.name.bytes();
            let first = chars.next().unwrap();
            if !(first == b'_' || first.is_ascii_alphabetic())
                || !chars.all(|c| c == b'_' || c.is_ascii_alphanumeric())
            {
                return Err(format!("invalid config field name {}", f.name));
            }
            if !names.insert(&f.name) {
                return Err(format!("duplicate config field {}", f.name));
            }
            match &f.constraint {
                Some(Constraint::UintRange { min, max })
                    if min > max
                        || !matches!(f.config_type, Type::Uint32 | Type::Uint64)
                        || (f.config_type == Type::Uint32 && *max > u64::from(u32::MAX)) =>
                {
                    return Err("invalid unsigned range".into());
                }
                Some(Constraint::StringEnum(values))
                    if f.config_type != Type::String
                        || values.is_empty()
                        || values.iter().collect::<BTreeSet<_>>().len() != values.len()
                        || values
                            .iter()
                            .any(|v| f.max_size != 0 && v.len() > f.max_size as usize) =>
                {
                    return Err("invalid string enum".into());
                }
                _ => {}
            }
            if let Some(v) = &f.default_value {
                f.validate_value(v)?;
            }
        }
        Ok(())
    }
    pub fn fingerprint(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        fn mix(h: &mut u64, v: u64) {
            *h = (*h ^ v).wrapping_mul(0x0000_0100_0000_01b3);
        }
        for f in &self.fields {
            for b in f.name.bytes() {
                mix(&mut hash, u64::from(b));
            }
            mix(&mut hash, f.config_type as u64);
            mix(&mut hash, u64::from(f.required));
            mix(&mut hash, u64::from(f.max_size));
        }
        if self
            .fields
            .iter()
            .any(|f| f.scope != Scope::SystemOnly || f.constraint.is_some())
        {
            mix(&mut hash, 0x50524546);
            for f in &self.fields {
                mix(&mut hash, f.scope as u64);
                match &f.constraint {
                    None => mix(&mut hash, 0),
                    Some(Constraint::UintRange { min, max }) => {
                        mix(&mut hash, 1);
                        mix(&mut hash, *min);
                        mix(&mut hash, *max);
                    }
                    Some(Constraint::StringEnum(values)) => {
                        mix(&mut hash, 2);
                        for v in values {
                            mix(&mut hash, v.len() as u64);
                            for b in v.bytes() {
                                mix(&mut hash, u64::from(b));
                            }
                        }
                    }
                }
            }
        }
        hash
    }
    pub fn has_preferences(&self) -> bool {
        self.fields.iter().any(|f| f.scope != Scope::SystemOnly)
    }
    pub fn field(&self, name: &str) -> Result<&Field, Error> {
        self.fields
            .iter()
            .find(|f| f.name == name)
            .ok_or(Error::UnknownField)
    }
    pub fn validate_assignments(
        &self,
        values: &Assignments,
        user: bool,
        locks: &BTreeSet<String>,
    ) -> Result<(), Error> {
        self.validate().map_err(|_| Error::InvalidSchema)?;
        for (name, v) in values {
            let f = self.field(name)?;
            if user && (f.scope == Scope::SystemOnly || locks.contains(name)) {
                return Err(Error::AccessDenied);
            }
            f.validate_value(v).map_err(|_| Error::Bounds)?;
        }
        Ok(())
    }
    pub fn validate_locks(&self, locks: &BTreeSet<String>) -> Result<(), Error> {
        for name in locks {
            if self.field(name)?.scope != Scope::MdmLockable {
                return Err(Error::AccessDenied);
            }
        }
        Ok(())
    }
    pub fn decode_table(&self, bytes: &[u8]) -> Result<Assignments, Error> {
        let t = ConfigTable::parse(bytes).map_err(|_| Error::Malformed)?;
        if t.version() >= 2 && t.schema_fingerprint() != self.fingerprint() {
            return Err(Error::Fingerprint);
        }
        let mut values = Assignments::new();
        for (name, ty) in t.entries() {
            let value = match ty {
                ConfigType::Bool => Value::Bool(t.get_bool(name).map_err(|_| Error::Malformed)?),
                ConfigType::Uint32 => Value::Uint32(t.get_u32(name).map_err(|_| Error::Malformed)?),
                ConfigType::Uint64 => Value::Uint64(t.get_u64(name).map_err(|_| Error::Malformed)?),
                ConfigType::String => Value::String(
                    t.get_string(name)
                        .map_err(|_| Error::Malformed)?
                        .to_string(),
                ),
                ConfigType::Bytes => {
                    Value::Bytes(t.get_bytes(name).map_err(|_| Error::Malformed)?.to_vec())
                }
            };
            if values.insert(name.to_string(), value).is_some() {
                return Err(Error::Malformed);
            }
        }
        self.validate_assignments(&values, false, &BTreeSet::new())?;
        Ok(values)
    }
    pub fn encode_table(&self, values: &Assignments, generation: u64) -> Result<Vec<u8>, Error> {
        self.validate_assignments(values, false, &BTreeSet::new())?;
        let owned = values
            .iter()
            .map(|(n, v)| Ok((n.as_str(), v.config_type().scalar()?, v.bytes())))
            .collect::<Result<Vec<_>, Error>>()?;
        let refs = owned
            .iter()
            .map(|(n, t, b)| (*n, *t, b.as_slice()))
            .collect::<Vec<_>>();
        encode_config_v2(self.fingerprint(), generation, &refs).map_err(|_| Error::Bounds)
    }
    pub fn resolve(
        &self,
        base: &Assignments,
        operator: &Assignments,
        user: &Assignments,
        locks: &BTreeSet<String>,
    ) -> Result<Assignments, Error> {
        self.validate_assignments(base, false, locks)?;
        self.validate_assignments(operator, false, locks)?;
        self.validate_locks(locks)?;
        // Locked preferences remain stored, but are not effective.
        self.validate_assignments(user, true, &BTreeSet::new())?;
        let mut result = self
            .fields
            .iter()
            .filter_map(|f| f.default_value.clone().map(|v| (f.name.clone(), v)))
            .collect::<Assignments>();
        result.extend(base.clone());
        result.extend(operator.clone());
        result.extend(
            user.iter()
                .filter(|(n, _)| !locks.contains(*n))
                .map(|(n, v)| (n.clone(), v.clone())),
        );
        if self
            .fields
            .iter()
            .any(|f| f.required && !result.contains_key(&f.name))
        {
            return Err(Error::MissingField);
        }
        Ok(result)
    }
}
fn number(kind: u8, b: &[u8]) -> Result<u64, Error> {
    if kind != 0 {
        return Err(Error::Malformed);
    }
    Ok(wire::varint(b)?.0)
}
fn message(kind: u8, b: &[u8]) -> Result<&[u8], Error> {
    if kind != 2 {
        return Err(Error::Malformed);
    }
    Ok(b)
}
fn text(kind: u8, b: &[u8]) -> Result<String, Error> {
    core::str::from_utf8(message(kind, b)?)
        .map(ToString::to_string)
        .map_err(|_| Error::Malformed)
}
fn decode_field(bytes: &[u8]) -> Result<Field, Error> {
    let mut f = Field::default();
    for item in wire::fields(bytes) {
        let (n, k, b) = item?;
        match n {
            1 => f.name = text(k, b)?,
            2 => f.config_type = Type::from_proto(number(k, b)?),
            3 => f.required = number(k, b)? != 0,
            4 => f.default_value = Some(Value::decode(message(k, b)?)?),
            5 => f.max_size = number(k, b)?.try_into().map_err(|_| Error::Malformed)?,
            6 => {
                f.scope = match number(k, b)? {
                    0 => Scope::SystemOnly,
                    1 => Scope::UserEditable,
                    2 => Scope::MdmLockable,
                    _ => return Err(Error::InvalidSchema),
                }
            }
            7 => f.display_name = text(k, b)?,
            8 => f.description = text(k, b)?,
            9 | 10 => {
                if f.constraint.is_some() {
                    return Err(Error::InvalidSchema);
                }
                let n_constraint = n;
                let mut min = 0;
                let mut max = 0;
                let mut values = Vec::new();
                for item in wire::fields(message(k, b)?) {
                    let (n, k, b) = item?;
                    if n == 1 {
                        if n_constraint == 10 {
                            values.push(text(k, b)?);
                        } else {
                            min = number(k, b)?;
                        }
                    } else if n == 2 {
                        max = number(k, b)?;
                    }
                }
                f.constraint = Some(if n == 9 {
                    Constraint::UintRange { min, max }
                } else {
                    Constraint::StringEnum(values)
                });
            }
            _ => {}
        }
    }
    Ok(f)
}
fn encode_field(f: &Field) -> Vec<u8> {
    let mut out = Vec::new();
    wire::bytes(&mut out, 1, f.name.as_bytes());
    wire::word(&mut out, 2, f.config_type as u64);
    wire::word(&mut out, 3, u64::from(f.required));
    if let Some(v) = &f.default_value {
        wire::bytes(&mut out, 4, &v.encode());
    }
    wire::word(&mut out, 5, u64::from(f.max_size));
    wire::word(&mut out, 6, f.scope as u64);
    wire::bytes(&mut out, 7, f.display_name.as_bytes());
    wire::bytes(&mut out, 8, f.description.as_bytes());
    let mut c = Vec::new();
    match &f.constraint {
        None => {}
        Some(Constraint::UintRange { min, max }) => {
            wire::word(&mut c, 1, *min);
            wire::word(&mut c, 2, *max);
            wire::bytes(&mut out, 9, &c);
        }
        Some(Constraint::StringEnum(values)) => {
            for v in values {
                wire::bytes(&mut c, 1, v.as_bytes());
            }
            wire::bytes(&mut out, 10, &c);
        }
    }
    out
}
