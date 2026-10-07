// SPDX-License-Identifier: Apache-2.0
//! DMX over the network and LED pixel mapping.
//!
//! - [`artnet`] and [`sacn`]: packet codecs written from the public Art-Net 4
//!   specification and ANSI E1.31-2018.
//! - [`mapping`]: fixture pixel positions, channel slots and sampling.
//! - [`net`]: the fixed-rate sender thread and datagram parsing.

pub mod artnet;
pub mod mapping;
pub mod net;
pub mod runtime;

pub use runtime::DmxRuntime;
pub mod sacn;
