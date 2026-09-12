//! Compile-time system font index.
//!
//! The pinned artifacts are structurally checked by the host tests. Keeping
//! their immutable metadata and Bazel-pinned digests here avoids reparsing and
//! hashing several megabytes on the emulated boot critical path.

pub struct Font {
    pub bytes: &'static [u8],
    pub digest: [u8; 32],
    pub family: &'static str,
    pub scripts: u8,
}

pub fn fonts() -> [Font; 5] {
    [
        Font {
            bytes: include_bytes!(env!("INTER")),
            digest: sha256(b"29160a80ff49ddcab2c97711247e08b1fab27a484a329ce8b813d820dc559031"),
            family: "Inter",
            scripts: 1,
        },
        Font {
            bytes: include_bytes!(env!("JETBRAINS_MONO")),
            digest: sha256(b"48715a42ec242c21e9f02692891e147d022299a52e48d5e413e1a942193ffeda"),
            family: "JetBrains Mono",
            scripts: 1,
        },
        Font {
            bytes: include_bytes!(env!("NOTO_SANS")),
            digest: sha256(b"bfb7bb691513f12e734dc346c03a03f784912432d7e3fa8e56efcf906fe86b3d"),
            family: "Noto Sans",
            scripts: 1,
        },
        Font {
            bytes: include_bytes!(env!("NOTO_ARABIC")),
            digest: sha256(b"63111b5b2e074dd48cc67692e0a2726d86ee94c1c37fe8598257b7b4e87e869e"),
            family: "Noto Sans Arabic",
            scripts: 2,
        },
        Font {
            bytes: include_bytes!(env!("NOTO_DEVANAGARI")),
            digest: sha256(b"9ce7b04f60e363d8870e5997744cf85cf69d38a4d7d129d364d92a3b14b461d7"),
            family: "Noto Sans Devanagari",
            scripts: 4,
        },
    ]
}

const fn sha256(hex: &[u8; 64]) -> [u8; 32] {
    let mut out = [0; 32];
    let mut index = 0;
    while index < out.len() {
        out[index] = nibble(hex[index * 2]) * 16 + nibble(hex[index * 2 + 1]);
        index += 1;
    }
    out
}

const fn nibble(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        _ => panic!("invalid SHA-256 literal"),
    }
}
