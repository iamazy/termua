//! Main application window (`TermuaWindow`).

mod actions;
mod render;
mod state;

pub(crate) use state::{TermuaWindow, WebShareIndicator};

#[cfg(test)]
mod tests;
