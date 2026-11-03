//! Types shared by the Musicus server and its clients.
//!
//! - [`dto`]: the JSON bodies of the REST API under `/api`.
//! - [`ink`]: the annotation model and its compact binary encoding, which
//!   clients send and receive as `application/octet-stream`.

pub mod dto;
pub mod ink;

pub use dto::*;
