//! Candidate archives become selectable only through registry activation.
pub(super) fn name(package: &str, generation: u64, digest: &[u8; 32]) -> alloc::string::String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut hash = alloc::string::String::new();
    for byte in digest.iter().take(16) {
        hash.push(HEX[(byte >> 4) as usize] as char);
        hash.push(HEX[(byte & 0xf) as usize] as char);
    }
    alloc::format!("{package}.generation_{generation}.{hash}")
}

pub(super) fn is_staged(package_id: &str) -> bool {
    let Some((package, suffix)) = package_id.rsplit_once(".generation_") else {
        return false;
    };
    let Some((generation, digest)) = suffix.split_once('.') else {
        return false;
    };
    !package.is_empty()
        && !generation.is_empty()
        && generation.bytes().all(|b| b.is_ascii_digit())
        && digest.len() == 32
        && digest.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::{is_staged, name};

    #[test]
    fn staged_candidates_are_distinct_from_preinstalled_packages() {
        assert!(is_staged(
            "bexos.app.sysui.generation_105.0123456789abcdef0123456789abcdef"
        ));
        assert!(is_staged(&name("bexos.app.sysui", u64::MAX, &[255; 32])));
        for package in [
            "bexos.app.sysui",
            "bexos.app.generation_tool",
            "bexos.app.generation_.0123456789abcdef0123456789abcdef",
            "bexos.app.generation_1.short",
            ".generation_1.0123456789abcdef0123456789abcdef",
        ] {
            assert!(!is_staged(package), "{package}");
        }
    }
}
