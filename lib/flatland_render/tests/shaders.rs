#[test]
fn kawase_shaders_validate() {
    let module = naga::front::wgsl::parse_str(include_str!("../src/kawase.wgsl")).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
    assert_eq!(
        module
            .entry_points
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        ["vertex", "downsample", "upsample"]
    );
}
