use super::Relocation;
pub fn relocation(kind: u32) -> Option<Relocation> {
    match kind {
        1027 => Some(Relocation::Relative),
        257 => Some(Relocation::Absolute),
        1025 => Some(Relocation::Absolute),
        1026 => Some(Relocation::Absolute),
        1028 => Some(Relocation::TlsModule),
        1029 => Some(Relocation::TlsOffset),
        1030 => Some(Relocation::ThreadOffset),
        _ => None,
    }
}
