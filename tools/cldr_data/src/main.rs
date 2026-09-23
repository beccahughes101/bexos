//! Hermetic CLDR-to-ICU blob export; source networking is disabled.
use icu_provider_export::{blob_exporter::BlobExporter, prelude::*};
use icu_provider_source::SourceDataProvider;
fn main() {
    if let Err(e) = run() {
        eprintln!("cldr_data: {e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let output = args.next().ok_or("missing output")?;
    let source = args.next().ok_or("missing CLDR source")?;
    let icu_source = args.next().ok_or("missing ICU source")?;
    let locales = args
        .map(|s| s.parse::<DataLocaleFamily>())
        .collect::<Result<Vec<_>, _>>()?;
    if locales.is_empty() {
        return Err("missing locales".into());
    }
    let provider = SourceDataProvider::new_custom()
        .with_cldr(std::path::Path::new(&source))?
        .with_icuexport(std::path::Path::new(&icu_source))?;
    let mut markers = Vec::new();
    markers.extend_from_slice(icu::decimal::provider::MARKERS);
    markers.extend_from_slice(icu::list::provider::MARKERS);
    markers.extend_from_slice(icu::plurals::provider::MARKERS);
    markers.extend_from_slice(icu::locale::provider::MARKERS);
    markers.extend_from_slice(icu::calendar::provider::MARKERS);
    // Zone-name tables are not needed for the local civil date/time API.
    markers.extend_from_slice(&icu::datetime::provider::MARKERS[10..]);
    markers.push(
        icu::experimental::dimension::provider::currency::essentials::CurrencyEssentialsV1::INFO,
    );
    ExportDriver::new(
        locales,
        DeduplicationStrategy::None.into(),
        LocaleFallbacker::try_new_unstable(&provider)?,
    )
    .with_markers(markers)
    .export(
        &provider,
        BlobExporter::new_with_sink(Box::new(std::fs::File::create(output)?)),
    )?;
    Ok(())
}
