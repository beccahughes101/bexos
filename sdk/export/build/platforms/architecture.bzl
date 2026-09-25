"""Architecture selection used by the source-distributed SDK runtime."""

def guest_select(arm, x86):
    return select(
        {
            "//build/platforms:is_aarch64": arm,
            "//build/platforms:is_x86_64": x86,
        },
        no_match_error = "SDK target platform must be aarch64 or x86_64",
    )
