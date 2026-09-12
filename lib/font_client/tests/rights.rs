use bexos_font_client::acceptable_vmo_rights;

#[test]
fn accepts_only_transfer_read_map_vmos() {
    assert!(acceptable_vmo_rights(1 | 2 | 16));
    assert!(!acceptable_vmo_rights(2 | 16));
    assert!(!acceptable_vmo_rights(1 | 2));
    assert!(!acceptable_vmo_rights(1 | 2 | 4 | 16));
    assert!(!acceptable_vmo_rights(1 | 2 | 8 | 16));
    assert!(!acceptable_vmo_rights(1 | 2 | 16 | 32));
}
