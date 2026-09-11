//! Deliberately small decoder for scalar MMIO MOV, TEST and register-destination arithmetic instructions.
//! Unsupported encodings fail closed. Addresses are recomputed from guest
//! registers and must additionally match the nested-fault address at dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alu {
    Add,
    Or,
    And,
    Sub,
    Xor,
    Compare,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Alu {
        kind: Alu,
        register: u8,
        shift: u8,
        destination_bytes: u8,
    },
    Test {
        value: u64,
    },
    Read {
        register: u8,
        shift: u8,
        destination_bytes: u8,
    },
    Write {
        value: u64,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Access {
    pub address: u64,
    pub bytes: u8,
    pub length: u8,
    pub operation: Operation,
}
struct Cursor<'a> {
    code: &'a [u8],
    offset: usize,
}
impl Cursor<'_> {
    fn byte(&mut self) -> Option<u8> {
        let b = *self.code.get(self.offset)?;
        self.offset += 1;
        Some(b)
    }
    fn number(&mut self, count: usize) -> Option<u64> {
        let mut value = 0;
        for i in 0..count {
            value |= u64::from(self.byte()?) << (i * 8);
        }
        Some(value)
    }
}
/// Registers use architectural encoding order: RAX, RCX, RDX, RBX, RSP, RBP,
/// RSI, RDI, R8..R15. `long` refers to CS.L, not merely EFER.LMA.
pub fn decode(code: &[u8], rip: u64, registers: &[u64; 16], long: bool) -> Option<Access> {
    let mut cursor = Cursor {
        code: &code[..code.len().min(15)],
        offset: 0,
    };
    let (mut rex, mut operand16, mut address32) = (0, false, !long);
    let mut opcode = cursor.byte()?;
    if opcode == 0x66 {
        operand16 = true;
        opcode = cursor.byte()?;
    }
    if opcode == 0x67 && long {
        address32 = true;
        opcode = cursor.byte()?;
    }
    if long && opcode & 0xf0 == 0x40 {
        rex = opcode;
        opcode = cursor.byte()?;
    }
    let operand = if rex & 8 != 0 {
        8
    } else if operand16 {
        2
    } else {
        4
    };
    let mut destination_bytes = operand;
    let alu = match opcode {
        0x02 | 0x03 => Some(Alu::Add),
        0x0a | 0x0b => Some(Alu::Or),
        0x22 | 0x23 => Some(Alu::And),
        0x2a | 0x2b => Some(Alu::Sub),
        0x32 | 0x33 => Some(Alu::Xor),
        0x3a | 0x3b => Some(Alu::Compare),
        _ => None,
    };
    let test = matches!(opcode, 0x84 | 0x85 | 0xf6 | 0xf7);
    let (bytes, write, immediate) = match opcode {
        _ if alu.is_some() => (if opcode & 1 == 0 { 1 } else { operand }, false, false),
        0x84 | 0xf6 => (1, false, opcode == 0xf6),
        0x85 | 0xf7 => (operand, false, opcode == 0xf7),
        0x88 | 0xc6 => (1, true, opcode == 0xc6),
        0x89 | 0xc7 => (operand, true, opcode == 0xc7),
        0x8a => (1, false, false),
        0x8b => (operand, false, false),
        0x0f => match cursor.byte()? {
            0xb6 => (1, false, false),
            0xb7 => (2, false, false),
            _ => return None,
        },
        _ => return None,
    };
    if opcode == 0x8a || alu.is_some() && bytes == 1 {
        destination_bytes = 1;
    }
    let modrm = cursor.byte()?;
    let mode = modrm >> 6;
    if mode == 3 || immediate && modrm & 0x38 != 0 {
        return None;
    }
    let mut register = ((modrm >> 3) & 7) | ((rex & 4) << 1);
    let mut shift = 0;
    if bytes == 1 && rex == 0 && register >= 4 && opcode != 0x0f {
        register -= 4;
        shift = 8;
    }
    let rm = modrm & 7;
    let mut address = 0u64;
    let mut rip_relative = false;
    let mut displacement32 = mode == 2;
    if rm == 4 {
        let sib = cursor.byte()?;
        let index = ((sib >> 3) & 7) | ((rex & 2) << 2);
        if index != 4 {
            address =
                address.wrapping_add(registers[index as usize].wrapping_shl(u32::from(sib >> 6)));
        }
        let base = sib & 7;
        if mode == 0 && base == 5 {
            displacement32 = true;
        } else {
            address = address.wrapping_add(registers[(base | ((rex & 1) << 3)) as usize]);
        }
    } else if mode == 0 && rm == 5 {
        displacement32 = true;
        rip_relative = long && !address32;
    } else {
        address = registers[(rm | ((rex & 1) << 3)) as usize];
    }
    if displacement32 {
        address = address.wrapping_add(cursor.number(4)? as i32 as i64 as u64);
    } else if mode == 1 {
        address = address.wrapping_add(cursor.byte()? as i8 as i64 as u64);
    }
    let operation = if let Some(kind) = alu {
        Operation::Alu {
            kind,
            register,
            shift,
            destination_bytes,
        }
    } else if immediate {
        let value = cursor.number(usize::from(bytes.min(4)))?;
        let value = if bytes == 8 {
            value as i32 as i64 as u64
        } else {
            value
        };
        if test {
            Operation::Test { value }
        } else {
            Operation::Write { value }
        }
    } else if test {
        Operation::Test {
            value: registers[register as usize] >> shift,
        }
    } else if write {
        Operation::Write {
            value: registers[register as usize] >> shift,
        }
    } else {
        Operation::Read {
            register,
            shift,
            destination_bytes,
        }
    };
    let operation = if let Operation::Write { value } = operation {
        Operation::Write {
            value: value & (u64::MAX >> (64 - u32::from(bytes) * 8)),
        }
    } else {
        operation
    };
    if rip_relative {
        address = address.wrapping_add(rip.checked_add(cursor.offset as u64)?);
    }
    if address32 {
        address = u64::from(address as u32);
    }
    Some(Access {
        address,
        bytes,
        length: cursor.offset as u8,
        operation,
    })
}
impl Access {
    pub fn alu_result(self, old: u64, memory: u64, flags: u64) -> Option<(u64, u64)> {
        let Operation::Alu {
            kind,
            shift,
            destination_bytes,
            ..
        } = self.operation
        else {
            return None;
        };
        let mask = u64::MAX >> (64 - u32::from(self.bytes) * 8);
        let sign = 1 << (u32::from(self.bytes) * 8 - 1);
        let left = (old >> shift) & mask;
        let right = memory & mask;
        let result = match kind {
            Alu::Add => left.wrapping_add(right),
            Alu::Sub | Alu::Compare => left.wrapping_sub(right),
            Alu::Or => left | right,
            Alu::And => left & right,
            Alu::Xor => left ^ right,
        } & mask;
        let mut status = logical_flags(flags, result, sign);
        match kind {
            Alu::Add => {
                if u128::from(left) + u128::from(right) > u128::from(mask) {
                    status |= 1;
                }
                if (!(left ^ right) & (left ^ result)) & sign != 0 {
                    status |= 1 << 11;
                }
                status |= (left ^ right ^ result) & 16;
            }
            Alu::Sub | Alu::Compare => {
                if left < right {
                    status |= 1;
                }
                if ((left ^ right) & (left ^ result)) & sign != 0 {
                    status |= 1 << 11;
                }
                status |= (left ^ right ^ result) & 16;
            }
            _ => {}
        }
        let value = if kind == Alu::Compare {
            old
        } else {
            Access {
                operation: Operation::Read {
                    register: 0,
                    shift,
                    destination_bytes,
                },
                ..self
            }
            .read_result(old, result)?
        };
        Some((value, status))
    }

    /// TEST changes only arithmetic status flags. AF is architecturally
    /// undefined; clear it consistently. All control flags remain unchanged.
    pub fn test_flags(self, flags: u64, memory: u64) -> Option<u64> {
        let Operation::Test { value } = self.operation else {
            return None;
        };
        let mask = u64::MAX >> (64 - u32::from(self.bytes) * 8);
        let result = value & memory & mask;
        Some(logical_flags(
            flags,
            result,
            1 << (u32::from(self.bytes) * 8 - 1),
        ))
    }

    pub fn read_result(self, old: u64, value: u64) -> Option<u64> {
        let Operation::Read {
            shift,
            destination_bytes,
            ..
        } = self.operation
        else {
            return None;
        };
        let mask = u64::MAX >> (64 - u32::from(self.bytes) * 8);
        let value = value & mask;
        Some(match destination_bytes {
            8 | 4 => value,
            2 => (old & !0xffff) | value,
            1 => (old & !(0xff << shift)) | (value << shift),
            _ => return None,
        })
    }
}
fn logical_flags(flags: u64, result: u64, sign: u64) -> u64 {
    (flags & !0x8d5)
        | if result == 0 { 1 << 6 } else { 0 }
        | if result & sign != 0 { 1 << 7 } else { 0 }
        | if (result as u8).count_ones() % 2 == 0 {
            1 << 2
        } else {
            0
        }
}
#[cfg(test)]
#[path = "mmio_tests.rs"]
mod tests;
