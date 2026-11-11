//! ricodex-cli: the command line front end. A thin wrapper over the same
//! engine library the app uses, proving the front end seam stays honest.
//!
//! Stub only: prints its name and version.

fn main() {
    println!("ricodex-cli {}", env!("CARGO_PKG_VERSION"));
}
