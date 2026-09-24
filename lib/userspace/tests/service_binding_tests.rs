use bexos_userspace::service_binding::ServiceBinding;

#[test]
fn service_binding_parses_allowed_methods_and_permissions() {
    let binding = ServiceBinding::parse("svc|Protocol|Public|1,4,7|READ,WRITE")
        .expect("metadata should parse");

    assert_eq!(binding.service, "svc");
    assert!(binding.protocol_is("Protocol"));
    assert_eq!(binding.capability, "Public");
    assert_eq!(binding.permission_values, vec!["READ", "WRITE"]);
    assert_eq!(binding.caller_package, None);
    assert_eq!(binding.caller_uid, None);
    assert!(binding.caller_foreground);
    assert_eq!(binding.provider_instance_id, None);
    assert!(binding.allows(1));
    assert!(binding.allows(4));
    assert!(!binding.allows(2));
}

#[test]
fn service_binding_treats_empty_methods_as_no_allowed_methods() {
    let binding =
        ServiceBinding::parse("svc|Protocol|Public||READ").expect("metadata should parse");

    assert!(!binding.allows(1));
}

#[test]
fn service_binding_rejects_malformed_methods() {
    assert!(ServiceBinding::parse("svc|Protocol|Public|abc|READ").is_none());
}

#[test]
fn service_binding_parses_caller_identity() {
    let binding = ServiceBinding::parse("svc|Protocol|Public|1|READ|com.example|42|bg|vpn-a")
        .expect("metadata should parse");

    assert_eq!(binding.caller_package.as_deref(), Some("com.example"));
    assert_eq!(binding.caller_uid, Some(42));
    assert!(!binding.caller_foreground);
    assert_eq!(binding.provider_instance_id.as_deref(), Some("vpn-a"));
}
