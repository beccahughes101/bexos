mod desktop;
mod service;
use service::Service;
fn main() {}
bexos_dioxus_guest::export!(Service with_types_in bexos_dioxus_guest);
