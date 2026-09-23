//! Keep ICU's experimental currency API private to this module.
use super::*;
use icu::experimental::dimension::currency::{CurrencyCode, formatter::CurrencyFormatter};
impl Formatting<'_> {
    pub fn currency(&self, value: &str, code: &str) -> Result<String, Error> {
        if value.len() > 256 || code.len() != 3 || !code.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(Error::Bounds);
        }
        let amount = value
            .parse::<fixed_decimal::Decimal>()
            .map_err(|_| Error::InvalidNumber)?;
        let code = CurrencyCode(code.parse().map_err(|_| Error::Malformed)?);
        let f = CurrencyFormatter::try_new_with_buffer_provider(
            &self.provider,
            self.locale()?.into(),
            Default::default(),
        )
        .map_err(|_| Error::Missing)?;
        Ok(f.format_fixed_decimal(&amount, &code).to_string())
    }
}
