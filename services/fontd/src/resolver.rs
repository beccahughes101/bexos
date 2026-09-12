use crate::index::Query;
use fonts_fidl::FontStatus;

pub trait MissingFontResolver {
    fn resolve(&mut self, query: &Query) -> Result<(), FontStatus>;
}

#[derive(Default)]
pub struct DisabledResolver;

impl MissingFontResolver for DisabledResolver {
    fn resolve(&mut self, _query: &Query) -> Result<(), FontStatus> {
        Err(FontStatus::NotFound)
    }
}
