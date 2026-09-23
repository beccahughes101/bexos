//! Shared validation for persisted settings and every locale IPC boundary.
#![no_std]
extern crate alloc;
use alloc::{
    string::{String, ToString},
    vec,
    vec::Vec,
};
use bexos_component_config::{ConfigTable, schema::Error};
pub use locale_fidl::{HourCycle, MeasurementSystem};
pub const PACKAGE_ID: &str = "bexos.locale.preferences";
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Settings {
    pub languages: Vec<String>,
    pub region: String,
    pub measurement: MeasurementSystem,
    pub hour_cycle: HourCycle,
    pub first_day_of_week: u8,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            languages: vec!["en-US".into()],
            region: "en-US".into(),
            measurement: MeasurementSystem::UsCustomary,
            hour_cycle: HourCycle::H12,
            first_day_of_week: 7,
        }
    }
}
pub fn canonical(tag: &str) -> Result<String, Error> {
    if tag.is_empty() || tag.len() > 32 || tag.trim() != tag {
        return Err(Error::Bounds);
    }
    let locale = tag
        .parse::<icu_locale_core::Locale>()
        .map_err(|_| Error::Malformed)?;
    let out = locale.to_string();
    if out.len() > 32 {
        return Err(Error::Bounds);
    }
    Ok(out)
}
impl Settings {
    pub fn from_table(bytes: &[u8]) -> Result<(u64, Self), Error> {
        let table = ConfigTable::parse(bytes).map_err(|_| Error::Malformed)?;
        let list = table
            .get_string("language_priority")
            .map_err(|_| Error::MissingField)?;
        let mut languages = Vec::new();
        for tag in list.split(',') {
            let tag = canonical(tag.trim())?;
            if languages.contains(&tag) || languages.len() == 8 {
                return Err(Error::Bounds);
            }
            languages.push(tag);
        }
        let override_region = table
            .get_string("override_region")
            .map_err(|_| Error::MissingField)?;
        let region = if override_region.is_empty() {
            languages[0].clone()
        } else {
            canonical(override_region)?
        };
        let measurement = match table
            .get_string("measurement_system")
            .map_err(|_| Error::MissingField)?
        {
            "metric" => MeasurementSystem::Metric,
            "us" => MeasurementSystem::UsCustomary,
            "uk" => MeasurementSystem::UkHybrid,
            _ => return Err(Error::Malformed),
        };
        let hour_cycle = match table
            .get_string("hour_cycle")
            .map_err(|_| Error::MissingField)?
        {
            "h11" => HourCycle::H11,
            "h12" => HourCycle::H12,
            "h23" => HourCycle::H23,
            "h24" => HourCycle::H24,
            _ => return Err(Error::Malformed),
        };
        let day = table
            .get_u32("first_day_of_week")
            .map_err(|_| Error::MissingField)?;
        if !(1..=7).contains(&day) {
            return Err(Error::Bounds);
        }
        Ok((
            table.generation(),
            Self {
                languages,
                region,
                measurement,
                hour_cycle,
                first_day_of_week: day as u8,
            },
        ))
    }
    pub fn from_wire(w: &locale_fidl::LocaleSettings<'_>) -> Result<Self, Error> {
        if w.language_chain.is_empty()
            || w.language_chain.len() > 8
            || !(1..=7).contains(&w.first_day_of_week)
        {
            return Err(Error::Bounds);
        }
        let mut languages = Vec::new();
        for i in 0..w.language_chain.len() {
            let l = canonical(w.language_chain.get(i).map_err(|_| Error::Malformed)?)?;
            if languages.contains(&l) {
                return Err(Error::Malformed);
            }
            languages.push(l);
        }
        Ok(Self {
            languages,
            region: canonical(w.region)?,
            measurement: w.measurement,
            hour_cycle: w.hour_cycle,
            first_day_of_week: w.first_day_of_week,
        })
    }
    pub fn with_wire<T>(&self, f: impl FnOnce(locale_fidl::LocaleSettings<'_>) -> T) -> T {
        let languages = self
            .languages
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        f(locale_fidl::LocaleSettings {
            language_chain: locale_fidl::WireStringVector::from_slice(&languages),
            region: &self.region,
            measurement: self.measurement,
            hour_cycle: self.hour_cycle,
            first_day_of_week: self.first_day_of_week,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_tags() {
        assert_eq!(canonical("es-mx").unwrap(), "es-MX");
        for v in ["", "../en", "en_US", " en-US"] {
            assert!(canonical(v).is_err());
        }
    }
    #[test]
    fn wire_roundtrip() {
        let s = Settings::default();
        s.with_wire(|w| assert_eq!(Settings::from_wire(&w).unwrap(), s));
    }
}

pub fn encode<T: locale_fidl::FidlEncode>(value: &T) -> Result<(Vec<u8>, Vec<u64>), Error> {
    let mut bytes = vec![0; 8192];
    let mut handles = [locale_fidl::HandleRef { raw: 0 }; 4];
    let result = value
        .encode(&mut bytes, &mut handles)
        .map_err(|_| Error::Malformed)?;
    bytes.truncate(result.bytes);
    Ok((
        bytes,
        handles[..result.handles].iter().map(|h| h.raw).collect(),
    ))
}
