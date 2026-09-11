use crate::{
    topology::{I2cPeripheralConfig, SpiPeripheralConfig},
    types::*,
};
use alloc::vec::Vec;

pub fn i2c_write_read(write: &[u8], read_length: usize) -> I2cBundle {
    I2cBundle {
        operations: alloc::vec![I2cOp::Write(write.to_vec()), I2cOp::Read(read_length)],
    }
}

pub fn i2c_write(bytes: &[u8]) -> I2cBundle {
    I2cBundle {
        operations: alloc::vec![I2cOp::Write(bytes.to_vec())],
    }
}

pub fn spi_exchange(bytes: &[u8], speed_hz: u32, mode: SpiMode) -> SpiBundle {
    SpiBundle {
        operations: alloc::vec![SpiOp::FullDuplex(bytes.to_vec())],
        speed_hz,
        mode,
    }
}

pub fn i2c_info(
    node_id: u64,
    controller_node_id: u64,
    cfg: &I2cPeripheralConfig,
) -> (u64, u64, u16, I2cAddressKind) {
    (
        controller_node_id,
        node_id,
        cfg.address.raw,
        cfg.address.kind,
    )
}

pub fn spi_info(
    node_id: u64,
    controller_node_id: u64,
    cfg: &SpiPeripheralConfig,
) -> (u64, u64, u32, u32, u32, u8) {
    (
        controller_node_id,
        node_id,
        cfg.chip_select,
        cfg.min_speed_hz,
        cfg.max_speed_hz,
        cfg.mode_mask,
    )
}

pub fn read_chunks(reads: &[ReadChunk]) -> Vec<Vec<u8>> {
    reads.iter().map(|read| read.bytes.clone()).collect()
}
