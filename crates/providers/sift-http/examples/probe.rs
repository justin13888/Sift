//! A live probe of the transport: one request to a real host over the platform's trust
//! store. Not a test — it needs the network, and a test that needs the network is a test
//! that fails on an aeroplane.
//!
//! `cargo run -p sift-http --example probe -- <host> <path>`
// Printing is what a diagnostic *is*. The workspace denies it everywhere else because
// a library that prints has no way to be quiet, and NFR-55 bounds what Sift writes
// down — neither applies to a command a person runs by hand and reads the output of.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use sift_provider::transport::{Request, Transport};

fn main() {
    let mut args = std::env::args().skip(1);
    let host = args.next().unwrap_or_else(|| "gmail.googleapis.com".into());
    let path = args
        .next()
        .unwrap_or_else(|| "/$discovery/rest?version=v1".into());
    let mut https = sift_http::Https::to(&host).expect("trust store");
    match https.exchange(&Request::new("GET", &path)) {
        Ok(r) => {
            println!("status {}", r.status);
            println!("content-type {:?}", r.header("content-type"));
            println!("body {} bytes", r.body.len());
            println!("wire (sent, received) = {:?}", https.wire_bytes());
            println!("connection reusable: {}", https.is_connected());
        }
        Err(e) => println!("failed: {e:?}"),
    }
}
