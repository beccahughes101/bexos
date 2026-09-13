//! ICU access confined to an immutable borrowed blob. No provider or formatter
//! borrowing the blob is exposed; every public result owns its data.
use bexos_locale_catalog::{Arguments, Error, Formatter, Value};
use bexos_locale_settings::Settings;
use icu::{
    locale::Locale,
    plurals::{PluralCategory, PluralOperands, PluralRules},
};
use icu_provider_blob::BlobDataProvider;
mod currency;
mod datetime;
mod number;

pub struct Formatting<'a> {
    provider: BlobDataProvider,
    pub settings: Settings,
    _borrow: core::marker::PhantomData<&'a [u8]>,
}
impl<'a> Formatting<'a> {
    pub fn new(bytes: &'a [u8], settings: Settings) -> Result<Self, Error> {
        if bytes.is_empty() || bytes.len() > 64 * 1024 * 1024 {
            return Err(Error::Bounds);
        }
        // ICU's static constructor avoids copying the blob. The phantom borrow
        // binds this private provider to bytes; all ICU payloads remain private
        // and are dropped before any method returns. No 'static value escapes.
        let data: &'static [u8] = unsafe { core::mem::transmute(bytes) };
        let provider =
            BlobDataProvider::try_new_from_static_blob(data).map_err(|_| Error::Malformed)?;
        Ok(Self {
            provider,
            settings,
            _borrow: core::marker::PhantomData,
        })
    }
    /// Region data absent from this build falls back to the shipped baseline.
    pub fn formatting_locale(&self) -> Result<Locale, Error> {
        let requested: Locale = self.settings.region.parse().map_err(|_| Error::Malformed)?;
        if icu::decimal::DecimalFormatter::try_new_with_buffer_provider(
            &self.provider,
            requested.clone().into(),
            Default::default(),
        )
        .is_ok()
        {
            Ok(requested)
        } else {
            "en-US".parse().map_err(|_| Error::Malformed)
        }
    }
    fn locale(&self) -> Result<Locale, Error> {
        self.formatting_locale()
    }
    pub fn direction(&self, language: &str) -> Result<bool, Error> {
        let locale: Locale = language.parse().map_err(|_| Error::Malformed)?;
        let dir =
            icu::locale::LocaleDirectionality::try_new_common_with_buffer_provider(&self.provider)
                .map_err(|_| Error::Missing)?;
        Ok(dir.is_right_to_left(&locale.id))
    }
    pub fn list(&self, items: &[&str], disjunction: bool) -> Result<String, Error> {
        if items.len() > 256 || items.iter().map(|s| s.len()).sum::<usize>() > 32768 {
            return Err(Error::Bounds);
        }
        let prefs = self.locale()?.into();
        let f = if disjunction {
            icu::list::ListFormatter::try_new_or_with_buffer_provider(
                &self.provider,
                prefs,
                Default::default(),
            )
        } else {
            icu::list::ListFormatter::try_new_and_with_buffer_provider(
                &self.provider,
                prefs,
                Default::default(),
            )
        }
        .map_err(|_| Error::Missing)?;
        Ok(f.format(items.iter().copied()).to_string())
    }
}
impl Formatter for Formatting<'_> {
    fn number(&self, value: &str, options: &Arguments) -> Result<String, Error> {
        self.format_number(value, options)
    }
    fn datetime(&self, value: &Value, options: &Arguments) -> Result<String, Error> {
        self.format_datetime(value, options)
    }
    fn plural(&self, language: &str, value: &str) -> Result<&'static str, Error> {
        if value.len() > 256 {
            return Err(Error::Bounds);
        }
        let locale: Locale = language.parse().map_err(|_| Error::Malformed)?;
        let rules =
            PluralRules::try_new_cardinal_with_buffer_provider(&self.provider, locale.into())
                .map_err(|_| Error::Missing)?;
        let operands = value
            .parse::<PluralOperands>()
            .map_err(|_| Error::InvalidNumber)?;
        Ok(match rules.category_for(operands) {
            PluralCategory::Zero => "zero",
            PluralCategory::One => "one",
            PluralCategory::Two => "two",
            PluralCategory::Few => "few",
            PluralCategory::Many => "many",
            PluralCategory::Other => "other",
        })
    }
}
fn option<'a>(options: &'a Arguments, name: &str) -> Option<&'a str> {
    options
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| match v {
            Value::Text(s) | Value::Number(s) => s.as_str(),
            Value::FormattedNumber { raw, .. } => raw.as_str(),
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    const DATA: &[u8] = include_bytes!(env!("TEST_CLDR"));
    fn engine(region: &str) -> Formatting<'static> {
        Formatting::new(
            DATA,
            Settings {
                region: region.into(),
                ..Settings::default()
            },
        )
        .unwrap()
    }
    #[test]
    fn regions_and_plural_languages_are_independent() {
        let f = engine("de-DE");
        assert_eq!(f.number("1234.56", &vec![]).unwrap(), "1.234,56");
        assert_eq!(f.plural("ru", "2").unwrap(), "few");
        assert_eq!(f.plural("ar", "2").unwrap(), "two");
        assert_eq!(
            engine("en-US").number("1234.56", &vec![]).unwrap(),
            "1,234.56"
        );
    }
    #[test]
    fn lists_currencies_direction() {
        let f = engine("en-US");
        assert_eq!(
            f.list(&["one", "two", "three"], false).unwrap(),
            "one, two, and three"
        );
        assert_eq!(f.currency("1234.56", "USD").unwrap(), "$1,234.56");
        assert!(f.direction("ar").unwrap());
        assert!(!f.direction("en-US").unwrap());
    }
    #[test]
    fn malformed_data() {
        assert!(Formatting::new(b"bad", Settings::default()).is_err());
    }
}
#[cfg(test)]
mod regression_tests {
    use super::*;
    use bexos_locale_settings::HourCycle;
    const DATA: &[u8] = include_bytes!(env!("TEST_CLDR"));
    fn format(region: &str, cycle: HourCycle) -> Formatting<'static> {
        Formatting::new(
            DATA,
            Settings {
                region: region.into(),
                hour_cycle: cycle,
                ..Settings::default()
            },
        )
        .unwrap()
    }
    #[test]
    fn plural_categories_use_translation_language() {
        let f = format("de-DE", HourCycle::H12);
        for (language, number, category) in [
            ("ru", "1", "one"),
            ("ru", "2", "few"),
            ("ru", "5", "many"),
            ("ru", "1.5", "other"),
            ("ar", "0", "zero"),
            ("ar", "1", "one"),
            ("ar", "2", "two"),
            ("ar", "7", "few"),
            ("ar", "15", "many"),
            ("ar", "100", "other"),
        ] {
            assert_eq!(f.plural(language, number).unwrap(), category);
        }
    }
    #[test]
    fn date_calendar_and_hour_cycle() {
        let value = Value::text("2026-09-13T00:05:09");
        let args = vec![("timeStyle".into(), Value::text("short"))];
        let h12 = format("en-US", HourCycle::H12)
            .datetime(&value, &args)
            .unwrap();
        assert!(h12.starts_with("12:05"), "{h12}");
        let h11 = format("en-US", HourCycle::H11)
            .datetime(&value, &args)
            .unwrap();
        assert!(h11.starts_with("0:05"), "{h11}");
        let h23 = format("de-DE", HourCycle::H23)
            .datetime(&value, &args)
            .unwrap();
        assert_eq!(h23, "00:05");
        let h24 = format("de-DE", HourCycle::H24)
            .datetime(&value, &args)
            .unwrap();
        assert_eq!(h24, "24:05");
        let f = format("en-US", HourCycle::H12);
        let args = vec![("dateStyle".into(), Value::text("full"))];
        let full = f.datetime(&value, &args).unwrap();
        assert!(full.contains("Sunday"), "{full}");
        let mut args = vec![("dateStyle".into(), Value::text("long"))];
        let gregorian = f.datetime(&value, &args).unwrap();
        args.push(("calendar".into(), Value::text("buddhist")));
        let buddhist = f.datetime(&value, &args).unwrap();
        assert_ne!(gregorian, buddhist);
        assert!(buddhist.contains("2569"), "{buddhist}");
        assert_eq!(
            f.datetime(
                &Value::number(0),
                &vec![("dateStyle".into(), Value::text("long"))]
            )
            .unwrap(),
            "January 1, 1970"
        );
    }
    #[test]
    fn formatting_limits_and_precision() {
        let f = format("de-DE", HourCycle::H23);
        assert_eq!(
            f.number(
                "1.2",
                &vec![("minimumFractionDigits".into(), Value::number(3))]
            )
            .unwrap(),
            "1,200"
        );
        assert_eq!(
            f.number(
                "1234.567",
                &vec![
                    ("maximumFractionDigits".into(), Value::number(2)),
                    ("useGrouping".into(), Value::text("false"))
                ]
            )
            .unwrap(),
            "1234,57"
        );
        assert!(f.number("NaN", &vec![]).is_err());
        assert!(f.currency("1", "usd").is_err());
        assert!(f.list(&vec!["x"; 257], false).is_err());
        assert!(f.currency("12.50", "EUR").unwrap().contains("12,50"));
        assert_eq!(f.list(&["eins", "zwei"], false).unwrap(), "eins und zwei");
        assert_eq!(
            format("ko-KR", HourCycle::H12)
                .number("1234.5", &vec![])
                .unwrap(),
            "1,234.5"
        );
    }
}
