//! Services create profiles using their own manifest permission. Appd only
//! attaches the transferred profile to the service thread it already manages.
use bexos_userspace::{Channel, Memory};
use kernel_fidl::*;
pub const PROFILE_MESSAGE: &[u8] = b"bexos.graphics.profile";
pub fn request(control: Channel) -> Result<(), Status> {
    request_period(control, 16_667_000)
}
pub fn request_period(control: Channel, period_ns: u64) -> Result<(), Status> {
    if period_ns != 16_667_000 && period_ns != 8_333_000 {
        return Err(Status::ErrInvalidArgs);
    }
    let response: ProfileProviderCreateProfileResponse = bexos_userspace::ipc::kernel_call(
        11,
        "CreateProfile",
        PROFILE_PROVIDER_PUBLIC_METHODS,
        &ProfileProviderCreateProfileRequest {
            info: SchedulingProfileInfo::Deadline(DeadlineProfile {
                capacity_ns: 3_000_000,
                deadline_ns: if period_ns == 8_333_000 {
                    7_500_000
                } else {
                    16_667_000
                },
                period_ns,
            }),
        },
    )?;
    if response.status != Status::Ok {
        return Err(response.status);
    }
    if let Err(error) = control.send(PROFILE_MESSAGE, &[response.profile_handle.raw]) {
        let _ = Memory::close(response.profile_handle.raw);
        return Err(error);
    }
    Ok(())
}
