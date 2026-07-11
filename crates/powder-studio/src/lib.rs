//! # powder-studio
//!
//! The Powder dashboard (`powder studio`) and the AI query-generation
//! plugin (`powder ai`), factored out of the CLI so a deployment can ship
//! without them and add them later — the CLI pulls this crate in behind its
//! `studio` cargo feature.
//!
//! - [`studio::serve`] — mobile-friendly web dashboard with token invites.
//! - [`ai`] — natural language → SQL against an OpenAI-compatible endpoint,
//!   with a FIFO admission gate for a shared model server.
//!
//! Configuration crosses as plain values ([`ai::AiConfig`]); reading
//! `powder.config.json` stays the CLI's job, so this crate has no file-layout
//! knowledge of its own.

pub mod ai;
pub mod studio;
