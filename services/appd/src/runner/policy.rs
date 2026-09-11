#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageTrustTier {
    PlatformCore,
    SystemHardware,
    StandardConsumer,
    VerifiedHighPerformance,
    DeveloperPowerUser,
}

impl PackageTrustTier {
    pub const fn allows_elf(self) -> bool {
        matches!(self, Self::PlatformCore | Self::SystemHardware)
    }
}
