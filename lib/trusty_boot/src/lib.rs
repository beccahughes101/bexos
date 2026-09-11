#![no_std]
//! Boot-time Trusty clients shared independently of monitor or SMC mechanics.
pub mod approval;
pub mod avb;
pub mod journal;
pub mod ql;
pub mod recovery;
pub mod selection;
pub mod storage;
