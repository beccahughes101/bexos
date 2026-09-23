mod font;
#[test]
fn remote_fixture_is_valid_and_cannot_match_a_built_in_family() {
    let bytes = font::remote_fixture(include_bytes!(env!("INTER")).to_vec());
    let faces = bexos_fontd::parser::parse(&bytes).unwrap();
    assert_eq!(faces.len(), 1);
    assert_eq!(faces[0].family, "Probe");
    assert_eq!(faces[0].weight, 400);
}
