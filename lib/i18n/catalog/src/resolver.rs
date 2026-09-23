use crate::{Catalog, Error, kind};
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
pub type Arguments = Vec<(String, Value)>;
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Text(String),
    Number(String),
    FormattedNumber { raw: String, text: String },
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Self::Text(v)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Self::Text(v.into())
    }
}
macro_rules! numbers {($($t:ty),*)=>{$(impl From<$t> for Value{fn from(v:$t)->Self{Self::number(v)}})*};}
numbers!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, f32, f64);
impl Value {
    pub fn number(value: impl core::fmt::Display) -> Self {
        Self::Number(format!("{value}"))
    }
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }
    fn raw(&self) -> &str {
        match self {
            Self::Text(v) | Self::Number(v) => v,
            Self::FormattedNumber { raw, .. } => raw,
        }
    }
    fn numeric(&self) -> bool {
        !matches!(self, Self::Text(_))
    }
}
pub trait Formatter {
    fn number(&self, value: &str, options: &Arguments) -> Result<String, Error>;
    fn datetime(&self, value: &Value, options: &Arguments) -> Result<String, Error>;
    fn plural(&self, language: &str, value: &str) -> Result<&'static str, Error>;
}
pub struct Resolver<'a, F> {
    pub catalog: Catalog<'a>,
    pub languages: Vec<String>,
    pub formatter: F,
}
impl<F: Formatter> Resolver<'_, F> {
    pub fn resolve(&self, key: &str, args: &Arguments) -> Result<String, Error> {
        if key.len() > 256
            || self.languages.len() > 8
            || self.languages.iter().any(|l| l.len() > 32)
            || args.len() > 256
            || args.iter().any(|(k, v)| {
                k.len() > 256
                    || match v {
                        Value::Text(s) | Value::Number(s) => s.len() > 65536,
                        Value::FormattedNumber { raw, text } => {
                            raw.len().saturating_add(text.len()) > 65536
                        }
                    }
            })
        {
            return Err(Error::Bounds);
        }
        let mut chain = Vec::<&str>::new();
        for language in &self.languages {
            let mut tag = language.as_str();
            loop {
                if !chain.contains(&tag) {
                    chain.push(tag);
                }
                let Some((parent, _)) = tag.rsplit_once('-') else {
                    break;
                };
                tag = parent;
            }
        }
        let baseline = self.catalog.default_locale();
        if !chain.contains(&baseline) {
            chain.push(baseline);
        }
        // First look for the requested plural category across the language
        // chain. Only then use Fluent's explicit default variants.
        for defaults in [false, true] {
            for language in &chain {
                let Ok(root) = self.catalog.find(language, key) else {
                    continue;
                };
                let mut budget = 4096;
                match self
                    .eval(root, args, language, defaults, 0, &mut budget)
                    .and_then(|v| self.display(v))
                {
                    Ok(text) => return Ok(text),
                    Err(Error::Missing | Error::MissingVariant) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        Err(Error::Missing)
    }
    pub fn text(&self, key: &str, args: &Arguments) -> String {
        self.resolve(key, args)
            .unwrap_or_else(|_| format!("⟪{key}⟫"))
    }
    fn display(&self, v: Value) -> Result<String, Error> {
        match v {
            Value::Text(s) | Value::FormattedNumber { text: s, .. } => Ok(s),
            Value::Number(n) => self.formatter.number(&n, &Vec::new()),
        }
    }
    fn eval(
        &self,
        index: u32,
        args: &Arguments,
        language: &str,
        defaults: bool,
        depth: usize,
        budget: &mut usize,
    ) -> Result<Value, Error> {
        if depth >= 64 || *budget == 0 {
            return Err(Error::EvaluationLimit);
        }
        *budget -= 1;
        let n = self.catalog.node(index)?;
        if matches!(n.kind, kind::TEXT | kind::NUMBER) && n.b > 65536 {
            return Err(Error::EvaluationLimit);
        }
        let child = |i, a, b: &mut usize| self.eval(i, a, language, defaults, depth + 1, b);
        match n.kind {
            kind::TEXT => Ok(Value::Text(self.catalog.string(n.a, n.b)?.to_string())),
            kind::NUMBER => Ok(Value::Number(self.catalog.string(n.a, n.b)?.to_string())),
            kind::VARIABLE => args
                .iter()
                .find(|(k, _)| Ok(k.as_str()) == self.catalog.string(n.a, n.b))
                .map(|(_, v)| v.clone())
                .ok_or(Error::MissingArgument),
            kind::PATTERN => {
                let mut out = String::new();
                for i in self.catalog.edges(n.a, n.b)? {
                    let text = self.display(child(i?, args, budget)?)?;
                    if out.len().saturating_add(text.len()) > 65536 {
                        return Err(Error::EvaluationLimit);
                    }
                    out.push_str(&text);
                }
                Ok(Value::Text(out))
            }
            kind::MESSAGE | kind::TERM | kind::FUNCTION => {
                let name = self.catalog.string(n.a, n.b)?;
                let mut params = Vec::new();
                for i in self.catalog.edges(n.c, n.d)? {
                    let a = self.catalog.node(i?)?;
                    params.push((
                        self.catalog.string(a.a, a.b)?.to_string(),
                        child(a.c, args, budget)?,
                    ));
                }
                if n.kind == kind::FUNCTION {
                    let v = &params
                        .iter()
                        .find(|(k, _)| k.is_empty())
                        .ok_or(Error::MissingArgument)?
                        .1;
                    return match name {
                        "NUMBER" if v.numeric() => Ok(Value::FormattedNumber {
                            raw: v.raw().to_string(),
                            text: self.formatter.number(v.raw(), &params)?,
                        }),
                        "DATETIME" => Ok(Value::Text(self.formatter.datetime(v, &params)?)),
                        _ => Err(Error::Unsupported),
                    };
                }
                let root = self.catalog.find(language, name)?;
                child(
                    root,
                    if n.kind == kind::TERM { &params } else { args },
                    budget,
                )
            }
            kind::SELECT => {
                let selector = child(n.a, args, budget)?;
                let category = if selector.numeric() {
                    Some(self.formatter.plural(language, selector.raw())?)
                } else {
                    None
                };
                let mut plural = None;
                let mut default = None;
                for (index, edge) in self.catalog.edges(n.b, n.c)?.enumerate() {
                    let variant = self.catalog.node(edge?)?;
                    let key = self.catalog.node(variant.a)?;
                    let text = self.catalog.string(key.a, key.b)?;
                    if index == n.d as usize {
                        default = Some(variant.b);
                    }
                    if key.kind == kind::NUMBER && selector.numeric() {
                        if numeric_equal(text, selector.raw())? {
                            return child(variant.b, args, budget);
                        }
                    } else if !selector.numeric() && text == selector.raw() {
                        return child(variant.b, args, budget);
                    } else if category == Some(text) {
                        plural = Some(variant.b);
                    }
                }
                if let Some(root) = plural {
                    return child(root, args, budget);
                }
                if defaults || !selector.numeric() {
                    return child(default.ok_or(Error::Malformed)?, args, budget);
                }
                Err(Error::MissingVariant)
            }
            _ => Err(Error::Malformed),
        }
    }
}

fn numeric_equal(left: &str, right: &str) -> Result<bool, Error> {
    fn parts(s: &str) -> Result<(bool, &str, &str), Error> {
        if s.is_empty() || s.len() > 256 {
            return Err(Error::InvalidNumber);
        }
        let negative = s.starts_with('-');
        let s = s.strip_prefix('-').unwrap_or(s);
        let (integer, fraction) = s.split_once('.').unwrap_or((s, ""));
        if integer.is_empty()
            || !integer.bytes().all(|c| c.is_ascii_digit())
            || !fraction.bytes().all(|c| c.is_ascii_digit())
        {
            return Err(Error::InvalidNumber);
        }
        let integer = integer.trim_start_matches('0');
        let fraction = fraction.trim_end_matches('0');
        Ok((
            negative && (!integer.is_empty() || !fraction.is_empty()),
            integer,
            fraction,
        ))
    }
    Ok(parts(left)? == parts(right)?)
}
