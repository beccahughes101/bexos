//! Bounded, allocation-backed ELF64 loading without a standard library.
#![no_std]
extern crate alloc;
use alloc::{
    collections::BTreeMap,
    rc::Rc,
    string::{String, ToString},
    vec::Vec,
};
pub mod arch;
mod dynamic;
pub mod load;
mod mapping;
mod parse;
mod relocation;
pub mod runtime;
mod symbols;
pub mod tls;
mod types;
mod util;
use arch::{Machine, Relocation};
pub use dynamic::{DynamicLibrary, LibraryPolicy, executable_scope, link_executable};
use load::{ElfLoadError, LoadPlan, PAGE_SIZE, RIGHTS_EXECUTE, RIGHTS_READ, RIGHTS_WRITE};
use parse::*;
pub use parse::{dynamic_library_span, library_tls};
use relocation::*;
use symbols::*;
pub use types::*;
use util::*;
pub use util::{align_down, align_up_to, page_round};

#[cfg(test)]
mod tests;

pub use symbols::executable_runtime_anchors;
