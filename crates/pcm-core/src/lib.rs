//! PC Monitor 2.0 core: the data model and every calculation that doesn't
//! need Windows, so it is tested anywhere. Readers of the real hardware live
//! in `pcm-win`; they hand raw numbers here.

pub mod config;
pub mod frames;
pub mod gpu_engines;
pub mod history;
pub mod merge;
pub mod model;
pub mod nvme;
pub mod processes;
pub mod rate;
pub mod sensors;
pub mod session;
pub mod smbios;
pub mod throttle;
pub mod update;
