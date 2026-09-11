use bexos_opener_store::{
    HandlerId, HandlerRegistration, MemoryOpenerRegistry, OpenKind, OpenerScope, ResolveOutcome,
    ResolvedHandler,
};

#[test]
fn resolves_system_handlers_for_system_callers() {
    let mut registry = MemoryOpenerRegistry::new();
    registry
        .register(OpenerScope::System, pdf_handler("bexos.viewer", "main"))
        .unwrap();

    assert_eq!(
        registry.resolve(OpenerScope::System, OpenKind::Mime("application/pdf")),
        ResolveOutcome::Selected(ResolvedHandler {
            package: "bexos.viewer".to_string(),
            process: "main".to_string(),
        })
    );
}

#[test]
fn user_handlers_override_system_and_defaults_select_one_match() {
    let mut registry = MemoryOpenerRegistry::new();
    registry
        .register(OpenerScope::System, pdf_handler("bexos.viewer", "main"))
        .unwrap();
    registry
        .register(OpenerScope::User(42), pdf_handler("user.viewer", "main"))
        .unwrap();
    registry
        .register(OpenerScope::User(42), pdf_handler("user.reader", "main"))
        .unwrap();

    assert!(matches!(
        registry.resolve(OpenerScope::User(42), OpenKind::Mime("application/pdf")),
        ResolveOutcome::PromptPendingUser(_)
    ));

    registry
        .set_user_default(
            42,
            "mime:application/pdf",
            HandlerId::new("user.reader", "main"),
        )
        .unwrap();
    assert_eq!(
        registry.resolve(OpenerScope::User(42), OpenKind::Mime("application/pdf")),
        ResolveOutcome::Selected(ResolvedHandler {
            package: "user.reader".to_string(),
            process: "main".to_string(),
        })
    );
}

#[test]
fn wildcard_mime_scheme_interface_and_uninstall_are_supported() {
    let mut registry = MemoryOpenerRegistry::new();
    registry
        .register(
            OpenerScope::System,
            HandlerRegistration {
                package: "bexos.media".to_string(),
                process: "main".to_string(),
                schemes: vec!["media".to_string()],
                domains: vec!["example.com".to_string()],
                mime_types: vec!["image/*".to_string()],
                interfaces: vec!["bexos.ui.WebViewEngine".to_string()],
                domains_verified: false,
            },
        )
        .unwrap();

    assert!(matches!(
        registry.resolve(OpenerScope::System, OpenKind::Mime("image/png")),
        ResolveOutcome::Selected(_)
    ));
    assert!(matches!(
        registry.resolve(OpenerScope::System, OpenKind::Url("media://play")),
        ResolveOutcome::Selected(_)
    ));
    assert!(matches!(
        registry.resolve(
            OpenerScope::System,
            OpenKind::Interface("bexos.ui.WebViewEngine")
        ),
        ResolveOutcome::Selected(_)
    ));
    assert_eq!(
        registry.resolve(
            OpenerScope::System,
            OpenKind::Url("https://example.com/app")
        ),
        ResolveOutcome::NoHandler
    );

    registry.remove_package("bexos.media");
    assert_eq!(
        registry.resolve(OpenerScope::System, OpenKind::Mime("image/png")),
        ResolveOutcome::NoHandler
    );
}

#[test]
fn redb_store_persists_handlers_and_defaults() {
    use bexos_opener_store::persistent::OpenerStoreDb;
    use bexos_redb::mem::MemBlockStore;
    use std::sync::Arc;

    let store = Arc::new(MemBlockStore::new());
    let db = OpenerStoreDb::open(store.clone()).unwrap();
    db.register(OpenerScope::System, pdf_handler("bexos.viewer", "main"))
        .unwrap();
    db.register(OpenerScope::User(7), pdf_handler("user.viewer", "main"))
        .unwrap();
    db.set_user_default(
        7,
        "mime:application/pdf",
        HandlerId::new("user.viewer", "main"),
    )
    .unwrap();

    let reopened = OpenerStoreDb::open(store).unwrap();
    let snapshot = reopened.snapshot_memory().unwrap();
    assert_eq!(
        snapshot.resolve(OpenerScope::User(7), OpenKind::Mime("application/pdf")),
        ResolveOutcome::Selected(ResolvedHandler {
            package: "user.viewer".to_string(),
            process: "main".to_string(),
        })
    );

    reopened.remove_package("user.viewer").unwrap();
    assert_eq!(
        reopened
            .snapshot_memory()
            .unwrap()
            .resolve(OpenerScope::User(7), OpenKind::Mime("application/pdf")),
        ResolveOutcome::Selected(ResolvedHandler {
            package: "bexos.viewer".to_string(),
            process: "main".to_string(),
        })
    );
}

fn pdf_handler(package: &str, process: &str) -> HandlerRegistration {
    HandlerRegistration {
        package: package.to_string(),
        process: process.to_string(),
        schemes: Vec::new(),
        domains: Vec::new(),
        mime_types: vec!["application/pdf".to_string()],
        interfaces: Vec::new(),
        domains_verified: false,
    }
}

#[test]
fn platform_default_preserves_explicit_user_choice_and_ignores_stale_handlers() {
    let mut registry = MemoryOpenerRegistry::new();
    let register = |package: &str| HandlerRegistration {
        package: package.into(),
        process: "terminal".into(),
        schemes: vec![],
        domains: vec![],
        mime_types: vec![],
        interfaces: vec!["bexos.shell.ShellProvider".into()],
        domains_verified: false,
    };
    registry
        .register(OpenerScope::System, register("brush"))
        .unwrap();
    registry
        .register(OpenerScope::System, register("other"))
        .unwrap();
    let fallback = HandlerId::new("brush", "terminal");
    let kind = OpenKind::Interface("bexos.shell.ShellProvider");
    let selected = |package: &str| {
        ResolveOutcome::Selected(ResolvedHandler {
            package: package.into(),
            process: "terminal".into(),
        })
    };
    for scope in [OpenerScope::System, OpenerScope::User(42)] {
        assert_eq!(
            registry.resolve_with_default(scope, kind, Some(&fallback)),
            selected("brush")
        );
    }
    registry
        .set_user_default(
            42,
            "interface:bexos.shell.ShellProvider",
            HandlerId::new("other", "terminal"),
        )
        .unwrap();
    assert_eq!(
        registry.resolve_with_default(OpenerScope::User(42), kind, Some(&fallback)),
        selected("other")
    );
    assert_eq!(
        registry.resolve_with_default(OpenerScope::System, kind, Some(&fallback)),
        selected("brush")
    );
    let missing = HandlerId::new("absent", "terminal");
    assert!(matches!(
        registry.resolve_with_default(OpenerScope::System, kind, Some(&missing)),
        ResolveOutcome::PromptPendingUser(_)
    ));
}
