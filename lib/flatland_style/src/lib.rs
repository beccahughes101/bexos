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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StylesheetSet {
    user_agent: Vec<String>,
    user: Vec<String>,
    author: Vec<String>,
}

pub struct Theme {
    source: String,
    sheets: Vec<Arc<Stylesheet>>,
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
        Self::from_stylesheets(&StylesheetSet::author(css))
    }
    pub fn from_stylesheets(set: &StylesheetSet) -> Result<Self, Error> {
        let source = set.source();
        if source.len() > 256 * 1024 {
            return Err(Error::TooLarge);
        }
        // Servo defaults grid parsing off until its embedder supplies layout.
        // Flatland supplies bounded Taffy grid layout and validates projection.
        stylo_static_prefs::set_pref!("layout.grid.enabled", true);
        let lock = SharedRwLock::new();
        let sheets = Self::parse_set(set, &lock);
        Ok(Self {
            source,
            sheets,
            lock,
            revision: 1,
        })
    }
    fn parse(css: &str, origin: Origin, lock: &SharedRwLock, index: usize) -> Arc<Stylesheet> {
        let url = UrlExtraData(Arc::new(format!("bexos:theme/{index}").parse().unwrap()));
        Arc::new(Stylesheet::from_str(
            css,
            url,
            origin,
            Arc::new(lock.wrap(MediaList::empty())),
            lock.clone(),
            None,
            None,
            QuirksMode::NoQuirks,
            AllowImportRules::No,
        ))
    }
    fn parse_set(set: &StylesheetSet, lock: &SharedRwLock) -> Vec<Arc<Stylesheet>> {
        let mut sheets = Vec::new();
        for css in &set.user_agent {
            sheets.push(Self::parse(css, Origin::UserAgent, lock, sheets.len()));
        }
        for css in &set.user {
            sheets.push(Self::parse(css, Origin::User, lock, sheets.len()));
        }
        for css in &set.author {
            sheets.push(Self::parse(css, Origin::Author, lock, sheets.len()));
        }
        sheets
    }
    pub fn update(&mut self, css: &str) -> Result<bool, Error> {
        if self.source == css {
            return Ok(false);
        }
        self.update_stylesheets(&StylesheetSet::author(css))
    }
    pub fn update_stylesheets(&mut self, set: &StylesheetSet) -> Result<bool, Error> {
        let source = set.source();
        if source.len() > 256 * 1024 {
            return Err(Error::TooLarge);
        }
        if self.source == source {
            return Ok(false);
        }
        self.sheets = Self::parse_set(set, &self.lock);
        self.source = source;
        self.revision += 1;
        Ok(true)
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn stylesheet(&self) -> &Arc<Stylesheet> {
        &self.sheets[0]
    }
    pub fn stylesheets(&self) -> &[Arc<Stylesheet>] {
        &self.sheets
    }
    pub fn lock(&self) -> &SharedRwLock {
        &self.lock
    }
    pub fn source(&self) -> &str {
        &self.source
    }
}

impl StylesheetSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn author(css: &str) -> Self {
        let mut set = Self::new();
        set.push_author(css);
        set
    }

    pub fn set_user_agent(&mut self, css: &str) {
        self.user_agent.clear();
        self.push_user_agent(css);
    }

    pub fn push_user_agent(&mut self, css: &str) {
        self.user_agent.push(css.into());
    }

    pub fn set_user(&mut self, css: &str) {
        self.user.clear();
        self.push_user(css);
    }

    pub fn push_user(&mut self, css: &str) {
        self.user.push(css.into());
    }

    pub fn push_author(&mut self, css: &str) {
        self.author.push(css.into());
    }

    pub fn source(&self) -> String {
        let mut out = String::new();
        for (origin, sheets) in [
            ("user-agent", &self.user_agent),
            ("user", &self.user),
            ("author", &self.author),
        ] {
            for sheet in sheets {
                out.push_str("/* ");
                out.push_str(origin);
                out.push_str(" */\n");
                out.push_str(sheet);
                out.push('\n');
            }
        }
        out
    }
}
