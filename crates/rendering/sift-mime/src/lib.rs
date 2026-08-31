//! Pipeline stages 1 and 2 — MIME parse and part selection.
//!
//! # Every byte here is attacker-controlled
//!
//! The sender chose all of it, is unauthenticated by default, and has unlimited attempts at
//! no cost. Two requirements shape everything below:
//!
//! - **NFR-19.** Malformed or hostile input MUST NEVER crash the process, and parse failure
//!   degrades to FR-9's raw source view. Sift is resident and under D-2 that process holds
//!   every account's sync state, the mutation queue, and the shell.
//! - **The limits register.** Exceeding a parse limit **rejects the message to the raw
//!   view; it does not truncate.** Truncating hands the sanitizer's output contract to the
//!   attacker — a document cut mid-tree is one whose structure the sender chose by choosing
//!   where the cap fell, which is the parse-differential primitive I8 exists to close.
//!
//! # Structure first
//!
//! **Sift MUST NOT fetch whole messages.** A message carrying a 40 MB attachment costs a
//! few kilobytes until the user asks for the attachment, so parsing yields a *structure*
//! and the bytes of one selected part — never the whole tree materialised.

pub mod parse;
pub mod select;
