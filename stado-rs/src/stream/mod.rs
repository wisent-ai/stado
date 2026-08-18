//! Interactive display sessions on fleet hosts, and the stream a client sees.
//!
//! The fleet already places batch work and inference on a GPU host. This is the
//! third thing a board can be asked for: render an interactive session now, and
//! hand the frames to whoever is holding the keyboard. Nothing about it is
//! borrowable over a network — the renderer runs where the board is — so the
//! fleet's job is to provision that renderer and say where to point the client.
//!
//! `schema` owns the declaration and its rules; `crate::deploy::stream` owns the
//! host side; `crate::cli::stream` is the operator surface.

pub mod schema;
