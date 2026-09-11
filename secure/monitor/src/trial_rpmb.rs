//! Resident UART endpoint for cold trials. Only complete RPMB read requests
//! can reach hardware; programming and authenticated writes remain fenced.
const FRAME: usize = 512;
const MAX: usize = 8 * FRAME;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Protocol,
    Transport,
}

pub struct Uart {
    tx: [u8; MAX + 4],
    rx: [u8; MAX],
    tx_len: usize,
    rx_len: usize,
    rx_at: usize,
    lcr: u8,
    reads: u64,
    blocked: u64,
}
impl Uart {
    pub const fn new() -> Self {
        Self {
            tx: [0; MAX + 4],
            rx: [0; MAX],
            tx_len: 0,
            rx_len: 0,
            rx_at: 0,
            lcr: 0,
            reads: 0,
            blocked: 0,
        }
    }
    pub fn reads(&self) -> u64 {
        self.reads
    }
    pub fn blocked(&self) -> u64 {
        self.blocked
    }
    pub fn port(
        &mut self,
        offset: u16,
        write: Option<u8>,
        mut exchange: impl FnMut(&[u8], &mut [u8]) -> Result<(), Error>,
    ) -> Result<u8, Error> {
        if let Some(value) = write {
            match offset {
                3 => self.lcr = value,
                0 if self.lcr & 0x80 == 0 => {
                    if self.rx_at != self.rx_len || self.tx_len == self.tx.len() {
                        return Err(Error::Protocol);
                    }
                    self.tx[self.tx_len] = value;
                    self.tx_len += 1;
                    if self.tx_len >= 4 {
                        let read = u16::from_le_bytes(self.tx[..2].try_into().unwrap()) as usize;
                        let written =
                            u16::from_le_bytes(self.tx[2..4].try_into().unwrap()) as usize;
                        if !(1..=8).contains(&read) || !(1..=8).contains(&written) {
                            return Err(Error::Protocol);
                        }
                        if self.tx_len == 4 + written * FRAME {
                            self.rx_len = read * FRAME;
                            self.rx_at = 0;
                            self.rx[..self.rx_len].fill(0);
                            let kind =
                                u16::from_be_bytes(self.tx[4 + 510..4 + 512].try_into().unwrap());
                            if written == 1 && matches!(kind, 2 | 4) {
                                exchange(&self.tx[..self.tx_len], &mut self.rx[..self.rx_len])?;
                                self.reads += 1;
                            } else {
                                // A synthetic failure cannot authenticate a successful
                                // mutation. No byte of this request reaches hardware.
                                for frame in self.rx[..self.rx_len].chunks_exact_mut(FRAME) {
                                    frame[508..510].copy_from_slice(&5u16.to_be_bytes());
                                    frame[510..512]
                                        .copy_from_slice(&kind.wrapping_shl(8).to_be_bytes());
                                }
                                self.blocked += 1;
                            }
                            self.tx[..self.tx_len].fill(0);
                            self.tx_len = 0;
                        }
                    }
                }
                _ => (),
            }
            Ok(0)
        } else {
            Ok(match offset {
                0 if self.lcr & 0x80 == 0 && self.rx_at < self.rx_len => {
                    let value = self.rx[self.rx_at];
                    self.rx[self.rx_at] = 0;
                    self.rx_at += 1;
                    value
                }
                3 => self.lcr,
                5 => 0x20 | u8::from(self.rx_at < self.rx_len),
                6 => 0x80,
                _ => 0,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(kind: u16) -> [u8; FRAME + 4] {
        let mut bytes = [0; FRAME + 4];
        bytes[..4].copy_from_slice(&[1, 0, 1, 0]);
        bytes[514..516].copy_from_slice(&kind.to_be_bytes());
        bytes
    }
    #[test]
    fn only_complete_read_requests_reach_hardware() {
        for kind in [0, 1, 2, 3, 4, 5, 0xffff] {
            let mut uart = Uart::new();
            let request = request(kind);
            let mut forwarded = 0;
            for byte in request {
                uart.port(0, Some(byte), |input, output| {
                    assert_eq!(input, request);
                    assert_eq!(output.len(), FRAME);
                    output.fill(0xa5);
                    forwarded += 1;
                    Ok(())
                })
                .unwrap();
            }
            assert_eq!(forwarded, usize::from(matches!(kind, 2 | 4)));
            assert_eq!(uart.blocked(), u64::from(!matches!(kind, 2 | 4)));
            for _ in 0..FRAME {
                uart.port(0, None, |_, _| {
                    panic!("read cannot initiate hardware exchange")
                })
                .unwrap();
            }
            assert_eq!(uart.port(5, None, |_, _| unreachable!()), Ok(0x20));
        }
    }
    #[test]
    fn truncated_malformed_and_configuration_bytes_never_escape() {
        let mut uart = Uart::new();
        for (port, value) in [(3, 0x80), (0, 1), (1, 0), (3, 3)] {
            uart.port(port, Some(value), |_, _| panic!("configuration escaped"))
                .unwrap();
        }
        assert_eq!(uart.tx_len, 0);
        for byte in request(3).into_iter().take(100) {
            uart.port(0, Some(byte), |_, _| panic!("partial request escaped"))
                .unwrap();
        }
        let mut uart = Uart::new();
        for byte in [0, 0, 1] {
            uart.port(0, Some(byte), |_, _| unreachable!()).unwrap();
        }
        assert_eq!(
            uart.port(0, Some(0), |_, _| unreachable!()),
            Err(Error::Protocol)
        );
    }
}
