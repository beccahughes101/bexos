pub const OPC_IDENTIFY: u8 = 0x06;
pub const CNS_IDENTIFY_NAMESPACE: u32 = 0x00;
pub const CNS_IDENTIFY_CONTROLLER: u32 = 0x01;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IdentifyCommand {
    pub namespace_id: u32,
    pub controller_or_namespace: u32,
    pub prp1: u64,
}

impl IdentifyCommand {
    pub const fn namespace(namespace_id: u32, prp1: u64) -> Self {
        Self {
            namespace_id,
            controller_or_namespace: CNS_IDENTIFY_NAMESPACE,
            prp1,
        }
    }
}
