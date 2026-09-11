//! Domain-local UART registers and unfinished console output. The sink and
//! domain label belong to the current monitor, never to an imported pointer.
use crate::state_wire::InvalidState;

pub struct Uart {
    registers: [u8; 8],
    bytes: [u8; 1024],
    used: usize,
}
impl Default for Uart {
    fn default() -> Self {
        Self::new()
    }
}
impl Uart {
    pub const STATE_BYTES: usize = 1040;
    pub const fn new() -> Self {
        Self {
            registers: [0; 8],
            bytes: [0; 1024],
            used: 0,
        }
    }
    pub fn read(&self, offset: u16) -> Option<u8> {
        Some(match offset {
            2 => 1,
            5 => 0x60,
            6 => 0xb0,
            _ => *self.registers.get(usize::from(offset))?,
        })
    }
    /// The returned line is emitted exactly once by the caller before the
    /// next guest entry. Checkpointing occurs only after that emission.
    pub fn write(&mut self, offset: u16, byte: u8) -> Result<Option<&[u8]>, InvalidState> {
        if offset >= 8 {
            return Err(InvalidState);
        }
        if offset != 0 || self.registers[3] & 0x80 != 0 {
            self.registers[usize::from(offset)] = byte;
            return Ok(None);
        }
        self.bytes[self.used] = byte;
        self.used += 1;
        if byte == b'\n' || self.used == self.bytes.len() {
            let used = self.used;
            self.used = 0;
            Ok(Some(&self.bytes[..used]))
        } else {
            Ok(None)
        }
    }
    /// Nested state, covered by the authenticated/protected outer checkpoint.
    pub fn snapshot(&self, output: &mut [u8]) -> Result<(), InvalidState> {
        if output.len() != Self::STATE_BYTES {
            return Err(InvalidState);
        }
        output.fill(0);
        output[..8].copy_from_slice(&self.registers);
        output[8..12].copy_from_slice(&(self.used as u32).to_le_bytes());
        output[16..16 + self.used].copy_from_slice(&self.bytes[..self.used]);
        Ok(())
    }
    pub fn restore_protected(&mut self, input: &[u8]) -> Result<(), InvalidState> {
        if input.len() != Self::STATE_BYTES || input[12..16] != [0; 4] {
            return Err(InvalidState);
        }
        let used = u32::from_le_bytes(input[8..12].try_into().unwrap()) as usize;
        if used >= 1024
            || input[16 + used..].iter().any(|byte| *byte != 0)
            || input[16..16 + used].contains(&b'\n')
        {
            return Err(InvalidState);
        }
        let mut restored = Self::new();
        restored.registers.copy_from_slice(&input[..8]);
        restored.bytes[..used].copy_from_slice(&input[16..16 + used]);
        restored.used = used;
        *self = restored;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_lines_and_divisor_latch_survive_without_replaying_emitted_lines() {
        let mut source = Uart::new();
        for byte in b"old\npartial" {
            source.write(0, *byte).unwrap();
        }
        source.write(3, 0x80).unwrap();
        source.write(0, 12).unwrap();
        let mut record = [0; Uart::STATE_BYTES];
        source.snapshot(&mut record).unwrap();
        let mut target = Uart::new();
        target.restore_protected(&record).unwrap();
        assert_eq!(target.read(0), Some(12));
        target.write(3, 3).unwrap();
        assert_eq!(
            target.write(0, b'\n').unwrap(),
            Some(b"partial\n".as_slice())
        );
        target.snapshot(&mut record).unwrap();
        assert!(record[8..].iter().all(|byte| *byte == 0));
    }
    #[test]
    fn malformed_partial_line_leaves_existing_output_owned_by_old_instance() {
        let mut uart = Uart::new();
        uart.write(0, b'x').unwrap();
        let mut record = [0; Uart::STATE_BYTES];
        uart.snapshot(&mut record).unwrap();
        record[1024] = 1;
        assert_eq!(uart.restore_protected(&record), Err(InvalidState));
        assert_eq!(uart.write(0, b'\n').unwrap(), Some(b"x\n".as_slice()));
    }
}
