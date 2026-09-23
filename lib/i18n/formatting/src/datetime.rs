use super::*;
use bexos_locale_settings::HourCycle;
use icu::datetime::{
    DateTimeFormatter, DateTimeFormatterPreferences,
    fieldsets::{T, YMD, YMDE, YMDET, YMDT},
    input::DateTime,
    options::TimePrecision,
};
use icu::locale::preferences::extensions::unicode::keywords;
impl Formatting<'_> {
    pub fn format_datetime(&self, value: &Value, args: &Arguments) -> Result<String, Error> {
        let date = match value {
            Value::Text(s) => s.clone(),
            Value::Number(ms) | Value::FormattedNumber { raw: ms, .. } => {
                // Fluent numeric dates are milliseconds since the Unix epoch, UTC.
                let ms = ms.parse::<i64>().map_err(|_| Error::InvalidNumber)?;
                let days = ms.div_euclid(86_400_000);
                let seconds = ms.rem_euclid(86_400_000) / 1000;
                let d = icu::calendar::Date::from_rata_die(
                    icu::calendar::types::RataDie::new(719163 + days),
                    icu::calendar::Iso,
                );
                format!(
                    "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
                    d.year().extended_year(),
                    d.month().ordinal,
                    d.day_of_month().0,
                    seconds / 3600,
                    seconds / 60 % 60,
                    seconds % 60
                )
            }
        };
        let input = date
            .parse::<DateTime<icu::calendar::Iso>>()
            .map_err(|_| Error::Malformed)?;
        let mut prefs: DateTimeFormatterPreferences = self.locale()?.into();
        let cycle = option(args, "hourCycle").unwrap_or(match self.settings.hour_cycle {
            HourCycle::H11 => "h11",
            HourCycle::H12 => "h12",
            HourCycle::H23 => "h23",
            HourCycle::H24 => "h24",
        });
        prefs.hour_cycle = Some(match cycle {
            "h11" => keywords::HourCycle::H11,
            "h12" => keywords::HourCycle::H12,
            "h23" | "h24" => keywords::HourCycle::H23,
            _ => return Err(Error::Unsupported),
        });
        if let Some(calendar) = option(args, "calendar") {
            let locale: Locale = format!("und-u-ca-{calendar}")
                .parse()
                .map_err(|_| Error::Unsupported)?;
            prefs.calendar_algorithm = Some(
                DateTimeFormatterPreferences::from(locale)
                    .calendar_algorithm
                    .ok_or(Error::Unsupported)?,
            );
        }
        let date_style = option(args, "dateStyle");
        let time_style = option(args, "timeStyle");
        for style in [date_style, time_style].into_iter().flatten() {
            if !matches!(style, "short" | "medium" | "long" | "full") {
                return Err(Error::Unsupported);
            }
        }
        macro_rules! format_set {
            ($set:expr) => {{
                let f =
                    DateTimeFormatter::try_new_with_buffer_provider(&self.provider, prefs, $set)
                        .map_err(|_| Error::Missing)?;
                self.render_time(
                    &f.format(&input),
                    cycle == "h24" && input.time.hour.number() == 0,
                )
            }};
        }
        let precision = if time_style == Some("short") {
            TimePrecision::Minute
        } else {
            TimePrecision::Second
        };
        match (date_style, time_style) {
            (Some(style), None) => match style {
                "short" => format_set!(YMD::short()),
                "medium" => format_set!(YMD::medium()),
                "full" => format_set!(YMDE::long()),
                _ => format_set!(YMD::long()),
            },
            (None, Some(style)) => match style {
                "short" => format_set!(T::short().with_time_precision(precision)),
                "medium" => format_set!(T::medium().with_time_precision(precision)),
                _ => format_set!(T::long().with_time_precision(precision)),
            },
            _ => match date_style.unwrap_or("medium") {
                "short" => format_set!(YMDT::short().with_time_precision(precision)),
                "medium" => format_set!(YMDT::medium().with_time_precision(precision)),
                "full" => format_set!(YMDET::long().with_time_precision(precision)),
                _ => format_set!(YMDT::long().with_time_precision(precision)),
            },
        }
    }
    fn render_time(
        &self,
        formatted: &impl writeable::Writeable,
        midnight24: bool,
    ) -> Result<String, Error> {
        if !midnight24 {
            return Ok(formatted.write_to_string().into_owned());
        }
        let mut output = TimeParts::default();
        formatted
            .write_to_parts(&mut output)
            .map_err(|_| Error::Malformed)?;
        if let Some(range) = output.hour {
            let hour =
                self.format_number("24", &vec![("useGrouping".into(), Value::text("false"))])?;
            output.text.replace_range(range, &hour);
        }
        Ok(output.text)
    }
}
#[derive(Default)]
struct TimeParts {
    text: String,
    hour: Option<core::ops::Range<usize>>,
}
impl core::fmt::Write for TimeParts {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.text.push_str(s);
        Ok(())
    }
}
impl writeable::PartsWrite for TimeParts {
    type SubPartsWrite = Self;
    fn with_part(
        &mut self,
        part: writeable::Part,
        mut f: impl FnMut(&mut Self) -> core::fmt::Result,
    ) -> core::fmt::Result {
        let start = self.text.len();
        f(self)?;
        if part == icu::datetime::parts::HOUR {
            self.hour = Some(start..self.text.len());
        }
        Ok(())
    }
}
