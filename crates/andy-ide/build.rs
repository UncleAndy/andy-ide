//! Puts the commit the window was built from into its title.
//!
//! Two rounds in a row went into the same question — "is the window running a build with this change?" —
//! and neither side could answer it from the window itself. `git rev-parse --short HEAD` at build time is
//! enough: a title reading `andy-ide 0.1.0 (3e3f56f)` is not ambiguous, and a title with an older hash is
//! the answer by itself.
use std::process::Command;

fn main() {
    let commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=ANDY_IDE_COMMIT={commit}");
    // Committing or switching a branch must rebuild the title even though no source file moved.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads");
}
