use super::Relocation;
pub fn relocation(kind: u32) -> Option<Relocation> {
    match kind {
        8 => Some(Relocation::Relative),
        1 => Some(Relocation::Absolute),
        6 => Some(Relocation::Symbol),
        7 => Some(Relocation::Symbol),
        16 => Some(Relocation::TlsModule),
        17 => Some(Relocation::TlsOffset),
        18 => Some(Relocation::ThreadOffset),
        _ => None,
    }
}
