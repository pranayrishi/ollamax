// Build script: stamps the binary with the current git short SHA so
// `forge --version` and any deterministic-replay log can include it.
//
// Falls back to "unknown" gracefully if `git` isn't on PATH or the source
// isn't a git checkout (e.g., shipped tarball, vendored build).
//
// Re-runs only when HEAD moves, so it doesn't bust the build cache on every
// `cargo check`.

use std::process::Command;

fn main() {
    let sha = Command::new("git")
        .args(["rev-parse", "--short=10", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=FORGE_GIT_SHA={sha}");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads");
}
