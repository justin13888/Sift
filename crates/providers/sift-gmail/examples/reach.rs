//! A live reach of the provider, with whatever token is in the environment.
//!
//! With none — the ordinary case — it proves the whole request path against the real service:
//! the transport, the target construction, the status handling, and the parse of the
//! provider's own error document, which is what D-88's classifier turns on.
//!
//! `SIFT_ACCESS_TOKEN=... cargo run -p sift-gmail --example reach`

// Printing is what a diagnostic *is*. See the note in `schema-check`.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use sift_gmail::{Gmail, GmailError};
use sift_provider::adapter::{Adapter, RemoteFolderId};

fn main() {
    let token = std::env::var("SIFT_ACCESS_TOKEN").unwrap_or_else(|_| "not-a-token".into());
    let Ok(https) = sift_http::Https::to(sift_gmail::oauth::API_HOST) else {
        eprintln!("the platform's trust store could not be consulted");
        return;
    };
    let gmail = Gmail::new(https, &token);

    match gmail.enumerate_folders() {
        Ok(folders) => {
            println!("{} folder(s):", folders.len());
            for folder in folders {
                println!("  {:<22} {:?}", folder.id.0, folder.special_use);
            }
            match gmail.delta(&RemoteFolderId("INBOX".into()), None) {
                Ok(page) => println!(
                    "first backfill page: {} change(s), more={}",
                    page.changes.len(),
                    page.more
                ),
                Err(e) => println!("delta: {e}"),
            }
        }
        // With no token this is the expected answer, and reaching it means every part of the
        // path worked: TLS against the platform trust store, the target this adapter builds,
        // the status the transport carried across, and the classification of what came back.
        Err(GmailError::TokenRejected) => {
            println!("the provider refused the credential — which is the whole path working");
        }
        Err(e) => println!("failed: {e}"),
    }
    println!("wire (sent, received) = {:?}", gmail.wire_bytes());
}
