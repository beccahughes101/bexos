use alloc::vec::Vec;

#[cfg(not(bexos_arch_x86_64))]
use bexos_d1_uart::{Pl011Uart, UartMmio};
use bexos_debug_wire::{Frame, WireError, parse_frame};

pub trait ByteTransport {
    fn try_read_byte(&mut self) -> Option<u8>;
    fn read_byte(&mut self) -> u8;
    fn write_byte(&mut self, byte: u8);
    fn try_read_bytes(&mut self, bytes: &mut [u8]) -> usize {
        if let Some(slot) = bytes.first_mut() {
            if let Some(byte) = self.try_read_byte() {
                *slot = byte;
                return 1;
            }
        }
        0
    }
    fn write_bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.write_byte(*byte);
        }
    }
}

#[cfg(not(bexos_arch_x86_64))]
pub struct UartByteTransport<M> {
    uart: Pl011Uart<M>,
}

#[cfg(not(bexos_arch_x86_64))]
impl<M: UartMmio> UartByteTransport<M> {
    pub const fn new(uart: Pl011Uart<M>) -> Self {
        Self { uart }
    }
}

#[cfg(not(bexos_arch_x86_64))]
impl<M: UartMmio> ByteTransport for UartByteTransport<M> {
    fn try_read_byte(&mut self) -> Option<u8> {
        self.uart.try_read_byte().unwrap_or(None)
    }

    fn read_byte(&mut self) -> u8 {
        self.uart.read_byte_poll().unwrap()
    }

    fn write_byte(&mut self, byte: u8) {
        self.uart.write_byte_poll(byte).unwrap();
    }
    fn write_bytes(&mut self, bytes: &[u8]) {
        // Serialize a complete frame with kernel/userspace logging on PL011.
        // The MMIO fallback preserves compatibility with older ARM kernels.
        if bexos_userspace::syscall::console_frame(bytes).is_ok() {
            return;
        }
        for byte in bytes {
            self.write_byte(*byte);
        }
    }
}

pub fn read_frame<T: ByteTransport>(transport: &mut T) -> Result<Frame, WireError> {
    let mut buffer = Vec::new();
    loop {
        buffer.push(transport.read_byte());
        match parse_frame(&buffer) {
            Ok((frame, _)) => return Ok(frame),
            Err(WireError::Incomplete) => {}
            Err(error) => return Err(error),
        }
    }
}

pub fn write_frame<T: ByteTransport>(transport: &mut T, frame: &Frame) -> Result<(), WireError> {
    let mut bytes = Vec::new();
    frame.encode(&mut bytes)?;
    transport.write_bytes(&bytes);
    Ok(())
}

pub async fn read_frame_async<T: ByteTransport>(transport: &mut T) -> Result<Frame, WireError> {
    let mut buffer = Vec::new();
    loop {
        if let Some(byte) = transport.try_read_byte() {
            buffer.push(byte);
        } else {
            bexos_userspace::yield_now();
            continue;
        }
        match parse_frame(&buffer) {
            Ok((frame, _)) => return Ok(frame),
            Err(WireError::Incomplete) => {}
            Err(error) => return Err(error),
        }
    }
}

pub async fn write_frame_async<T: ByteTransport>(
    transport: &mut T,
    frame: &Frame,
) -> Result<(), WireError> {
    let mut bytes = Vec::new();
    frame.encode(&mut bytes)?;
    transport.write_bytes(&bytes);
    Ok(())
}

/// A retained private channel to the userspace virtio-console driver.
pub struct SerialByteTransport {
    channel: bexos_userspace::Channel,
}
impl SerialByteTransport {
    pub fn new(channel: bexos_userspace::Channel) -> Self {
        Self { channel }
    }
}
impl ByteTransport for SerialByteTransport {
    fn try_read_byte(&mut self) -> Option<u8> {
        let mut byte = [0];
        (self.try_read_bytes(&mut byte) == 1).then_some(byte[0])
    }
    fn try_read_bytes(&mut self, bytes: &mut [u8]) -> usize {
        use serial_fidl::{DeviceReadRequest, DeviceReadResponse, FidlDecode, FidlEncode, Status};
        if bytes.is_empty() {
            return 0;
        }
        let mut request = [0; 32];
        request[..8].copy_from_slice(&3u64.to_le_bytes());
        let Ok(encoded) = (DeviceReadRequest {
            max_bytes: bytes.len().min(4096) as u32,
        })
        .encode(&mut request[8..], &mut []) else {
            return 0;
        };
        if self
            .channel
            .send(&request[..8 + encoded.bytes], &[])
            .is_err()
        {
            return 0;
        }
        let Ok(response) = self.channel.recv_blocking() else {
            return 0;
        };
        let Ok(decoded) = DeviceReadResponse::decode(&response.bytes, &[]) else {
            return 0;
        };
        if decoded.status != Status::Ok || decoded.bytes.len() > bytes.len() {
            return 0;
        }
        bytes[..decoded.bytes.len()].copy_from_slice(decoded.bytes);
        decoded.bytes.len()
    }
    fn read_byte(&mut self) -> u8 {
        loop {
            if let Some(byte) = self.try_read_byte() {
                return byte;
            }
            bexos_userspace::yield_now();
        }
    }
    fn write_byte(&mut self, byte: u8) {
        self.write_bytes(&[byte]);
    }
    fn write_bytes(&mut self, bytes: &[u8]) {
        use serial_fidl::{
            DeviceWriteRequest, DeviceWriteResponse, FidlDecode, FidlEncode, Status,
        };
        let mut offset = 0;
        while offset < bytes.len() {
            let chunk = &bytes[offset..bytes.len().min(offset + 4096)];
            let mut request = [0; 8192];
            request[..8].copy_from_slice(&2u64.to_le_bytes());
            let encoded = DeviceWriteRequest { bytes: chunk }
                .encode(&mut request[8..], &mut [])
                .unwrap();
            if self
                .channel
                .send(&request[..8 + encoded.bytes], &[])
                .is_err()
            {
                return;
            }
            let Ok(response) = self.channel.recv_blocking() else {
                return;
            };
            if let Ok(response) = DeviceWriteResponse::decode(&response.bytes, &[]) {
                if response.status == Status::Ok && response.bytes_written as usize <= chunk.len() {
                    offset += response.bytes_written as usize;
                }
            }
            if offset < bytes.len() {
                bexos_userspace::yield_now();
            }
        }
    }
}
