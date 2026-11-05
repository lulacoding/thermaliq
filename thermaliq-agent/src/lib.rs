//! Agent logic (SPEC §7). Pure: no HAL, no I/O. The daemon wires it, and it only submits to
//! the engine (S1).

pub mod classifier;
pub mod decision;
pub mod inputs;
pub mod model;
pub mod optimiser;
