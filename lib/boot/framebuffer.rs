//! Optional firmware scanout metadata. No drawing belongs in the kernel.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)]
pub struct BootFramebuffer {
    pub address: u64,
    pub length: u64,
    pub width: u64,
    pub height: u64,
    pub stride: u64,
    pub format: u64,
}
impl BootFramebuffer {
    pub const NONE: Self = Self {
        address: 0,
        length: 0,
        width: 0,
        height: 0,
        stride: 0,
        format: 0,
    };
    pub fn valid(self) -> bool {
        self.address != 0
            && self.address % 4096 == 0
            && self.length > 0
            && self.length <= 64 * 1024 * 1024
            && self.length % 4096 == 0
            && self.address.checked_add(self.length).is_some()
            && self.width > 0
            && self.height > 0
            && self.width <= 4096
            && self.height <= 4096
            && self.stride % 4 == 0
            && self.stride >= self.width * 4
            && self
                .stride
                .checked_mul(self.height)
                .is_some_and(|n| n <= self.length)
            && matches!(self.format, 1 | 2)
    }
    pub const fn words(self) -> [u64; 6] {
        [
            self.address,
            self.length,
            self.width,
            self.height,
            self.stride,
            self.format,
        ]
    }
    pub fn from_words(w: [u64; 6]) -> Option<Self> {
        let f = Self {
            address: w[0],
            length: w[1],
            width: w[2],
            height: w[3],
            stride: w[4],
            format: w[5],
        };
        f.valid().then_some(f)
    }
}
/// Multiboot2 RGB framebuffer tag, validated against its containing byte slice.
pub fn multiboot_framebuffer(bytes: &[u8]) -> Option<BootFramebuffer> {
    let word = |i: usize| Some(u32::from_le_bytes(bytes.get(i..i + 4)?.try_into().ok()?));
    let total = word(0)? as usize;
    if total < 16 || total > bytes.len() {
        return None;
    }
    let mut cursor = 8;
    while cursor + 8 <= total {
        let kind = word(cursor)?;
        let len = word(cursor + 4)? as usize;
        if len < 8 || cursor.checked_add(len)? > total {
            return None;
        }
        if kind == 0 {
            break;
        }
        if kind == 8 && len >= 38 {
            let address = u64::from_le_bytes(bytes[cursor + 8..cursor + 16].try_into().ok()?);
            let stride = word(cursor + 16)? as u64;
            let width = word(cursor + 20)? as u64;
            let height = word(cursor + 24)? as u64;
            if bytes[cursor + 28] != 32 || bytes[cursor + 29] != 1 {
                return None;
            }
            let fields = &bytes[cursor + 32..cursor + 38];
            let format = match fields {
                [16, 8, 8, 8, 0, 8] => 1,
                [0, 8, 8, 8, 16, 8] => 2,
                _ => return None,
            };
            let length = crate::page_round(stride.checked_mul(height)?)?;
            return BootFramebuffer::from_words([address, length, width, height, stride, format]);
        }
        cursor = cursor.checked_add((len + 7) & !7)?;
    }
    None
}
/// Read a simple-framebuffer node from a bounded flattened device tree.
pub fn simple_framebuffer(dtb: &[u8]) -> Option<BootFramebuffer> {
    fn word(b: &[u8], o: usize) -> Option<u32> {
        Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?))
    }
    if word(dtb, 0)? != 0xd00dfeed {
        return None;
    }
    let total = word(dtb, 4)? as usize;
    if total > dtb.len() || total < 40 {
        return None;
    }
    let start = word(dtb, 8)? as usize;
    let strings = word(dtb, 12)? as usize;
    let slen = word(dtb, 32)? as usize;
    let len = word(dtb, 36)? as usize;
    let names = dtb.get(strings..strings.checked_add(slen)?)?;
    let data = dtb.get(start..start.checked_add(len)?)?;
    #[derive(Clone, Copy)]
    struct Node {
        address_cells: usize,
        size_cells: usize,
        child_address_cells: usize,
        child_size_cells: usize,
        fb: BootFramebuffer,
        compatible: bool,
        enabled: bool,
    }
    let base = Node {
        address_cells: 2,
        size_cells: 1,
        child_address_cells: 2,
        child_size_cells: 1,
        fb: BootFramebuffer::NONE,
        compatible: false,
        enabled: true,
    };
    let mut stack = [base; 32];
    let mut depth = 0;
    let mut p = 0;
    fn cells(b: &[u8], n: usize) -> Option<u64> {
        if n == 0 || n > 2 || b.len() < n * 4 {
            return None;
        }
        let mut v = 0;
        for c in b[..n * 4].chunks_exact(4) {
            v = (v << 32) | u32::from_be_bytes(c.try_into().ok()?) as u64;
        }
        Some(v)
    }
    while p + 4 <= data.len() {
        let tag = word(data, p)?;
        p += 4;
        match tag {
            1 => {
                if depth == 32 {
                    return None;
                }
                let parent = if depth == 0 { base } else { stack[depth - 1] };
                let mut node = base;
                node.address_cells = parent.child_address_cells;
                node.size_cells = parent.child_size_cells;
                stack[depth] = node;
                depth += 1;
                let n = data[p..].iter().position(|b| *b == 0)?;
                p = (p + n + 4) & !3;
            }
            2 => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                let n = stack[depth];
                if n.compatible && n.enabled {
                    let mut fb = n.fb;
                    fb.length = crate::page_round(fb.length)?;
                    if fb.valid() {
                        return Some(fb);
                    }
                }
            }
            3 => {
                if depth == 0 {
                    return None;
                }
                let len = word(data, p)? as usize;
                let off = word(data, p + 4)? as usize;
                p += 8;
                let value = data.get(p..p.checked_add(len)?)?;
                p = (p + len + 3) & !3;
                let name = names.get(off..)?;
                let name = &name[..name.iter().position(|b| *b == 0)?];
                let n = &mut stack[depth - 1];
                match name {
                    b"#address-cells" => n.child_address_cells = word(value, 0)? as usize,
                    b"#size-cells" => n.child_size_cells = word(value, 0)? as usize,
                    b"compatible" => {
                        n.compatible = value.split(|b| *b == 0).any(|v| v == b"simple-framebuffer")
                    }
                    b"status" => n.enabled = value == b"okay\0" || value == b"ok\0",
                    b"width" => n.fb.width = word(value, 0)? as u64,
                    b"height" => n.fb.height = word(value, 0)? as u64,
                    b"stride" => n.fb.stride = word(value, 0)? as u64,
                    b"format" => {
                        n.fb.format = match value {
                            b"a8r8g8b8\0" | b"x8r8g8b8\0" => 1,
                            b"a8b8g8r8\0" | b"x8b8g8r8\0" => 2,
                            _ => 0,
                        }
                    }
                    b"reg" => {
                        n.fb.address = cells(value, n.address_cells).unwrap_or(0);
                        n.fb.length = n
                            .address_cells
                            .checked_mul(4)
                            .and_then(|offset| value.get(offset..))
                            .and_then(|v| cells(v, n.size_cells))
                            .unwrap_or(0);
                    }
                    _ => {}
                }
            }
            4 => {}
            9 => break,
            _ => return None,
        }
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_ranges() {
        let mut f = BootFramebuffer {
            address: 0x90000000,
            length: 4096,
            width: 8,
            height: 8,
            stride: 32,
            format: 1,
        };
        assert!(f.valid());
        f.address = u64::MAX;
        assert!(!f.valid());
        assert!(multiboot_framebuffer(&[0; 40]).is_none());
        assert!(simple_framebuffer(&[0; 40]).is_none());
    }
    #[test]
    fn multiboot_rgb_tag_preserves_geometry() {
        let mut bytes = [0u8; 56];
        bytes[..4].copy_from_slice(&56u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&8u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&38u32.to_le_bytes());
        bytes[16..24].copy_from_slice(&0x90000000u64.to_le_bytes());
        for (offset, value) in [(24, 32u32), (28, 8), (32, 8), (52, 8)] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[36] = 32;
        bytes[37] = 1;
        bytes[40..46].copy_from_slice(&[16, 8, 8, 8, 0, 8]);
        let f = multiboot_framebuffer(&bytes).unwrap();
        assert_eq!(f.stride, 32);
        assert_eq!(f.length, 4096);
        assert_eq!(f.format, 1);
        bytes[36] = 24;
        assert!(multiboot_framebuffer(&bytes).is_none());
    }
    #[test]
    fn simple_framebuffer_ignores_unrelated_pci_reg_cells() {
        let mut data = std::vec::Vec::new();
        let mut names = std::vec::Vec::new();
        fn word(b: &mut std::vec::Vec<u8>, v: u32) {
            b.extend_from_slice(&v.to_be_bytes());
        }
        fn node(b: &mut std::vec::Vec<u8>, name: &[u8]) {
            word(b, 1);
            b.extend_from_slice(name);
            b.push(0);
            while b.len() % 4 != 0 {
                b.push(0);
            }
        }
        fn prop(b: &mut std::vec::Vec<u8>, names: &mut std::vec::Vec<u8>, name: &[u8], v: &[u8]) {
            word(b, 3);
            word(b, v.len() as u32);
            word(b, names.len() as u32);
            names.extend_from_slice(name);
            names.push(0);
            b.extend_from_slice(v);
            while b.len() % 4 != 0 {
                b.push(0);
            }
        }
        node(&mut data, b"");
        prop(
            &mut data,
            &mut names,
            b"#address-cells",
            &2u32.to_be_bytes(),
        );
        prop(&mut data, &mut names, b"#size-cells", &2u32.to_be_bytes());
        node(&mut data, b"pci");
        prop(
            &mut data,
            &mut names,
            b"#address-cells",
            &3u32.to_be_bytes(),
        );
        node(&mut data, b"device");
        prop(&mut data, &mut names, b"reg", &[0; 20]);
        word(&mut data, 2);
        word(&mut data, 2);
        node(&mut data, b"framebuffer");
        prop(
            &mut data,
            &mut names,
            b"compatible",
            b"simple-framebuffer\0",
        );
        let mut reg = std::vec::Vec::new();
        reg.extend_from_slice(&0x90000000u64.to_be_bytes());
        reg.extend_from_slice(&4096u64.to_be_bytes());
        prop(&mut data, &mut names, b"reg", &reg);
        for (name, value) in [
            (&b"width"[..], 8u32),
            (&b"height"[..], 8),
            (&b"stride"[..], 32),
        ] {
            prop(&mut data, &mut names, name, &value.to_be_bytes());
        }
        prop(&mut data, &mut names, b"format", b"x8r8g8b8\0");
        word(&mut data, 2);
        word(&mut data, 2);
        word(&mut data, 9);
        let mut dtb = std::vec![0;40];
        let total = 40 + data.len() + names.len();
        for (offset, value) in [
            (0, 0xd00dfeed),
            (4, total as u32),
            (8, 40),
            (12, (40 + data.len()) as u32),
            (32, names.len() as u32),
            (36, data.len() as u32),
        ] {
            dtb[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        dtb.extend(data);
        dtb.extend(names);
        assert_eq!(simple_framebuffer(&dtb).unwrap().address, 0x90000000);
    }
}
