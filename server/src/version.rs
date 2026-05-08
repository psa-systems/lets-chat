//! Build-time version metadata. Values are populated by `build.rs` from the
//! workspace package version, `git`, and the build clock. See `build.rs` for
//! how each value resolves and falls back.

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GIT_HASH: &str = env!("GIT_HASH");
pub const GIT_VERSION: &str = env!("GIT_VERSION");
pub const BUILD_DATE: &str = env!("BUILD_DATE");

/// Single-line human-readable banner. Used by `--version` output and the
/// startup tracing log.
pub fn banner(app: &str) -> String {
    format!("{app} {VERSION} ({GIT_VERSION}, commit {GIT_HASH}, built {BUILD_DATE})")
}
