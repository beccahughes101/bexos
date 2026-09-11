use bexos_flatland_style::*;
#[test]
fn theme_changes_invalidate_once() {
    let mut t = Theme::new(".window { opacity: .9; border-radius: 12px }").unwrap();
    assert!(!t.update(t.source().to_owned().as_str()).unwrap());
    assert_eq!(t.revision(), 1);
    assert!(t.update(".window { opacity: .5 }").unwrap());
    assert_eq!(t.revision(), 2);
}
