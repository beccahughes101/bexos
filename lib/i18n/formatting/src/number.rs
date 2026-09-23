use super::*;
use fixed_decimal::Decimal;
use icu::decimal::{
    DecimalFormatter,
    options::{DecimalFormatterOptions, GroupingStrategy},
};
impl Formatting<'_> {
    pub fn format_number(&self, value: &str, args: &Arguments) -> Result<String, Error> {
        if value.len() > 256 {
            return Err(Error::Bounds);
        }
        let mut number: Decimal = value.parse().map_err(|_| Error::InvalidNumber)?;
        let precision = |key| -> Result<Option<i16>, Error> {
            option(args, key)
                .map(|v| {
                    v.parse::<i16>()
                        .ok()
                        .filter(|v| (0..=20).contains(v))
                        .ok_or(Error::Bounds)
                })
                .transpose()
        };
        let min = precision("minimumFractionDigits")?;
        let max = precision("maximumFractionDigits")?;
        if min.zip(max).is_some_and(|(min, max)| min > max) {
            return Err(Error::Bounds);
        }
        if let Some(n) = max {
            number.round(-n);
        }
        if let Some(n) = min {
            number.absolute.pad_end(-n);
        }
        if let Some(n) = precision("minimumIntegerDigits")? {
            number.absolute.pad_start(n);
        }
        let mut options = DecimalFormatterOptions::default();
        options.grouping_strategy = Some(match option(args, "useGrouping") {
            None | Some("true" | "1") => GroupingStrategy::Auto,
            Some("false" | "0") => GroupingStrategy::Never,
            _ => return Err(Error::Unsupported),
        });
        let formatter = DecimalFormatter::try_new_with_buffer_provider(
            &self.provider,
            self.locale()?.into(),
            options,
        )
        .map_err(|_| Error::Missing)?;
        Ok(formatter.format(&number).to_string())
    }
}
