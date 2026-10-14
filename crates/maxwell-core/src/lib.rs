//! Maxwell PEQ without a UI: the filter compiler, Equalizer APO conversion,
//! the profile library, the headset protocol, and the session state machine.
//!
//! The desktop app (`src-tauri`) and the browser dev server both call
//! [`service::Service`]; neither touches the headset directly.

pub mod apo;
pub mod app;
pub mod audeze;
pub mod balance;
pub mod compile;
pub mod controls;
pub mod descriptor;
pub mod device;
pub mod dsp;
pub mod encoder;
pub mod error;
pub mod fmt;
pub mod hid;
pub mod http;
pub mod json;
pub mod paths;
pub mod profiles;
pub mod protocol;
pub mod remote;
pub mod service;
pub mod settings;
pub mod updates;
pub mod util;
pub mod vendor;
pub mod worker;

pub use error::{Error, Result};
