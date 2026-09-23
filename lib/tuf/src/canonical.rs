//! Strict JSON decoding and the integer-only canonical signing representation.
use crate::TufError;
use alloc::{collections::BTreeSet, string::String, vec::Vec};
use core::fmt;
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;

struct Strict(Value);
impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Json;
        impl<'de> Visitor<'de> for Json {
            type Value = Strict;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("unique-key integer JSON")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Strict, E> {
                Ok(Strict(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Strict, A::Error> {
                let mut values = Vec::new();
                while let Some(Strict(value)) = seq.next_element()? {
                    values.push(value);
                }
                Ok(Strict(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Strict, A::Error> {
                let mut values = serde_json::Map::new();
                let mut seen = BTreeSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !seen.insert(key.clone()) {
                        return Err(de::Error::custom("duplicate JSON key"));
                    }
                    let Strict(value) = map.next_value()?;
                    values.insert(key, value);
                }
                Ok(Strict(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(Json)
    }
}

pub fn parse(bytes: &[u8]) -> Result<Value, TufError> {
    serde_json::from_slice::<Strict>(bytes)
        .map(|value| value.0)
        .map_err(|_| TufError::InvalidJson)
}

pub fn encode(value: &Value) -> Result<Vec<u8>, TufError> {
    let mut out = Vec::new();
    write(value, &mut out)?;
    Ok(out)
}

fn string(value: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    for byte in value.bytes() {
        if byte == b'"' || byte == b'\\' {
            out.push(b'\\');
        }
        out.push(byte);
    }
    out.push(b'"');
}

fn write(value: &Value, out: &mut Vec<u8>) -> Result<(), TufError> {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(number) if number.is_i64() || number.is_u64() => {
            out.extend_from_slice(number.to_string().as_bytes())
        }
        Value::Number(_) => return Err(TufError::InvalidJson),
        Value::String(text) => string(text, out),
        Value::Array(values) => {
            out.push(b'[');
            for (i, value) in values.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write(value, out)?;
            }
            out.push(b']');
        }
        Value::Object(values) => {
            out.push(b'{');
            let mut keys: Vec<_> = values.keys().collect();
            keys.sort();
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                string(key, out);
                out.push(b':');
                write(&values[key], out)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}
