use super::*;
use bexos_locale_catalog::builder::Builder;
use std::{cell::RefCell, rc::Rc};
#[derive(Clone)]
struct TestBackend(Rc<RefCell<(u64, Settings)>>);
impl Formatter for TestBackend {
    fn number(&self, n: &str, _: &Arguments) -> Result<String, Error> {
        Ok(n.into())
    }
    fn datetime(&self, _: &Value, _: &Arguments) -> Result<String, Error> {
        Err(Error::Unsupported)
    }
    fn plural(&self, _: &str, _: &str) -> Result<&'static str, Error> {
        Ok("other")
    }
}
impl Backend for TestBackend {
    fn snapshot(&self) -> Result<(u64, Settings), Error> {
        Ok(self.0.borrow().clone())
    }
    fn direction(&self, language: &str) -> Result<&'static str, Error> {
        Ok(if language == "ar" { "rtl" } else { "ltr" })
    }
}
#[test]
fn generation_changes_translation_and_direction_without_resetting_application_state() {
    let mut b = Builder::default();
    for (language, text) in [("en-US", "Hello"), ("ar", "مرحبا")] {
        let root = b.string_node(bexos_locale_catalog::kind::TEXT, text);
        b.entry(language, "hello", root).unwrap();
    }
    let bytes = Box::leak(b.finish("en-US").unwrap().into_boxed_slice());
    let backend = TestBackend(Rc::new(RefCell::new((1, Settings::default()))));
    let mut context = LocaleContext::load(bytes, backend.clone()).unwrap();
    let app_state = (37, "typed input", 17, Some(22));
    assert_eq!(t!(context, "hello"), "Hello");
    assert!(!context.poll().unwrap());
    *backend.0.borrow_mut() = (
        2,
        Settings {
            languages: vec!["ar".into()],
            region: "de-DE".into(),
            ..Settings::default()
        },
    );
    assert!(context.poll().unwrap());
    assert_eq!(context.direction(), "rtl");
    assert_eq!(context.generation(), 2);
    assert_eq!(use_locale(&context).region, "de-DE");
    assert_eq!(t!(context, "hello"), "مرحبا");
    assert_eq!(app_state, (37, "typed input", 17, Some(22)));
    *backend.0.borrow_mut() = (1, Settings::default());
    assert!(!context.poll().unwrap());
    assert_eq!(t!(context, "hello"), "مرحبا");
}
