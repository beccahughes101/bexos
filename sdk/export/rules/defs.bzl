load(
    ":app.bzl",
    _bexos_app_archive = "bexos_app_archive",
    _bexos_app_manifest = "bexos_app_manifest",
    _bexos_driver = "bexos_driver",
    _bexos_native_app = "bexos_native_app",
    _bexos_service = "bexos_service",
    _bexos_wasm_app = "bexos_wasm_app",
)
load(":fidl.bzl", _bexos_fidl_rust_library = "bexos_fidl_rust_library")

bexos_app_archive = _bexos_app_archive
bexos_app_manifest = _bexos_app_manifest
bexos_driver = _bexos_driver
bexos_fidl_rust_library = _bexos_fidl_rust_library
bexos_native_app = _bexos_native_app
bexos_service = _bexos_service
bexos_wasm_app = _bexos_wasm_app

__all__ = [
    "bexos_app_archive",
    "bexos_app_manifest",
    "bexos_fidl_rust_library",
    "bexos_driver",
    "bexos_native_app",
    "bexos_service",
    "bexos_wasm_app",
]
