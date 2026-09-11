use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt::Write;

use bexos_d1_uart::{
    CR, DR, FBRD, FR, FR_TXFF, IBRD, ICR, IMSC, LCRH, Pl011Uart, UartError, UartMmio,
};

#[derive(Clone, Debug)]
struct MockMmio {
    registers: Vec<u32>,
    fr_reads: RefCell<VecDeque<u32>>,
    writes: Vec<(usize, u32)>,
}

impl MockMmio {
    fn new() -> Self {
        Self {
            registers: vec![0; 0x100 / 4],
            fr_reads: RefCell::new(VecDeque::new()),
            writes: Vec::new(),
        }
    }

    fn with_fr_reads(reads: &[u32]) -> Self {
        let mut mmio = Self::new();
        mmio.fr_reads = RefCell::new(reads.iter().copied().collect());
        mmio
    }

    fn register(&self, offset: usize) -> u32 {
        self.registers[offset / 4]
    }
}

impl UartMmio for MockMmio {
    fn read32(&self, offset: usize) -> Result<u32, UartError> {
        if offset >= self.registers.len() * 4 {
            return Err(UartError::MmioOutOfRange);
        }
        if offset == FR {
            return Ok(self
                .fr_reads
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| self.register(offset)));
        }
        Ok(self.register(offset))
    }

    fn write32(&mut self, offset: usize, value: u32) -> Result<(), UartError> {
        if offset >= self.registers.len() * 4 {
            return Err(UartError::MmioOutOfRange);
        }
        self.registers[offset / 4] = value;
        self.writes.push((offset, value));
        Ok(())
    }
}

#[test]
fn init_programs_expected_pl011_registers() {
    let mut uart = Pl011Uart::new(MockMmio::new());
    uart.init().unwrap();
    let mmio = uart.into_mmio();

    assert_eq!(mmio.writes[0], (CR, 0));
    assert!(mmio.writes.contains(&(ICR, 0x7ff)));
    assert!(mmio.writes.contains(&(IBRD, 1)));
    assert!(mmio.writes.contains(&(FBRD, 40)));
    assert!(mmio.writes.contains(&(LCRH, 0b11 << 5)));
    assert!(mmio.writes.contains(&(IMSC, 0)));
    assert_eq!(mmio.register(CR), (1 << 0) | (1 << 8) | (1 << 9));
}

#[test]
fn newline_output_inserts_carriage_return() {
    let mut uart = Pl011Uart::new(MockMmio::new());
    write!(uart, "a\n").unwrap();
    let bytes = uart
        .into_mmio()
        .writes
        .into_iter()
        .filter_map(|(offset, value)| (offset == DR).then_some(value as u8))
        .collect::<Vec<_>>();

    assert_eq!(bytes, b"a\r\n");
}

#[test]
fn tx_full_polling_waits_before_data_write() {
    let mmio = MockMmio::with_fr_reads(&[FR_TXFF, FR_TXFF, 0]);
    let mut uart = Pl011Uart::new(mmio);
    uart.write_byte_poll(b'x').unwrap();
    let mmio = uart.into_mmio();

    assert_eq!(mmio.writes.last(), Some(&(DR, b'x' as u32)));
}
