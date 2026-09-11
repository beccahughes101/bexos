#![no_std]
extern crate alloc;

pub mod bot;
pub mod descriptor;
pub mod hid;
pub mod migration;
pub mod policy;
pub mod transfer;
pub mod xhci;

pub use descriptor::{
    Configuration, DescriptorError, DeviceDescriptor, EndpointDescriptor, InterfaceDescriptor,
    ParsedDescriptors,
};
pub use policy::{ClassPolicy, InterfaceClass, PolicyDecision};
pub use transfer::{
    BufferRegistration, TransferError, TransferLimits, validate_buffer, validate_transfer,
};
