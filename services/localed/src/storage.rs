use bexos_locale_settings::Settings;
use bexos_userspace::{Channel, Memory, fs};
use locale_fidl::Status;
pub const READ_ONLY_RIGHTS: u32 = 1 | 2 | 16 | 32; // duplicate, read, map, transfer; never write
pub fn load(package: Channel) -> Result<(u64, u64), Status> {
    let file = fs::open(
        package,
        "data/locale/cldr.bexloc",
        fs_fidl::OpenFlags::RIGHT_READABLE.0,
    )
    .map_err(|_| Status::Io)?;
    let result = (|| {
        let len = fs::attributes(file).map_err(|_| Status::Io)?.size_bytes;
        if len == 0 || len > 64 * 1024 * 1024 {
            return Err(Status::InvalidArgs);
        }
        let bytes = fs::read(file, len).map_err(|_| Status::Io)?;
        if bytes.len() as u64 != len {
            return Err(Status::Io);
        }
        bexos_locale_formatting::Formatting::new(&bytes, Settings::default())
            .map_err(|_| Status::InvalidArgs)?;
        let writable = Memory::from_bytes(&bytes).map_err(|_| Status::Io)?;
        let result = Memory::duplicate(writable, READ_ONLY_RIGHTS).map_err(|_| Status::Io);
        let _ = Memory::close(writable);
        Ok((result?, len))
    })();
    let _ = fs::close(file);
    result
}
