//! Retained Stylo stylesheets shared by Flatland hosts. Parsed sheets can be
//! installed in a Stylist by the owner of the corresponding view tree.
use style::{
    context::QuirksMode,
    media_queries::MediaList,
    servo_arc::Arc,
    shared_lock::SharedRwLock,
    stylesheets::{AllowImportRules, Origin, Stylesheet, UrlExtraData},
};
pub use style::{properties::PropertyDeclarationBlock, stylist::Stylist};
pub mod dom;
mod grid;
pub mod metrics;
pub mod properties;
pub mod resolver;
pub use resolver::Resolver;
pub use style;
pub struct Theme {
    source: String,
    sheet: Arc<Stylesheet>,
    lock: SharedRwLock,
    revision: u64,
}
#[derive(Debug, PartialEq)]
pub enum Error {
    TooLarge,
    InvalidTree,
    InvalidViewport,
    UnsupportedLayout,
}
impl Theme {
    pub fn new(css: &str) -> Result<Self, Error> {
        if css.len() > 256 * 1024 {
            return Err(Error::TooLarge);
        }
        // Servo defaults grid parsing off until its embedder supplies layout.
        // Flatland supplies bounded Taffy grid layout and validates projection.
        stylo_static_prefs::set_pref!("layout.grid.enabled", true);
        let lock = SharedRwLock::new();
        let sheet = Self::parse(css, &lock);
        Ok(Self {
            source: css.into(),
            sheet,
            lock,
            revision: 1,
        })
    }
    fn parse(css: &str, lock: &SharedRwLock) -> Arc<Stylesheet> {
        let url = UrlExtraData(Arc::new("bexos:theme".parse().unwrap()));
        Arc::new(Stylesheet::from_str(
            css,
            url,
            Origin::Author,
            Arc::new(lock.wrap(MediaList::empty())),
            lock.clone(),
            None,
            None,
            QuirksMode::NoQuirks,
            AllowImportRules::No,
        ))
    }
    pub fn update(&mut self, css: &str) -> Result<bool, Error> {
        if css.len() > 256 * 1024 {
            return Err(Error::TooLarge);
        }
        if self.source == css {
            return Ok(false);
        }
        self.sheet = Self::parse(css, &self.lock);
        self.source = css.into();
        self.revision += 1;
        Ok(true)
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn stylesheet(&self) -> &Arc<Stylesheet> {
        &self.sheet
    }
    pub fn lock(&self) -> &SharedRwLock {
        &self.lock
    }
    pub fn source(&self) -> &str {
        &self.source
    }
}
