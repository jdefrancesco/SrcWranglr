//! Which SrcWranglr this is: the version in Cargo.toml and, when built from
//! a git checkout, the commit it was built from (`git describe`, recorded
//! by `build.rs`), so a build made between releases says so.

/// The version in Cargo.toml.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// `git describe --tags --always` at build time; empty outside a checkout.
const BUILD: &str = env!("SRCWRANGLR_BUILD");

/// This build's version, for `--version`, the start page and About.
pub fn version_line() -> String {
    describe(VERSION, BUILD)
}

/// `version`, with `build` after it unless it is just that version's tag.
fn describe(version: &str, build: &str) -> String {
    let is_release = build.strip_prefix('v') == Some(version);
    if build.is_empty() || is_release {
        version.to_string()
    } else {
        format!("{version} ({build})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_build_is_just_its_version() {
        assert_eq!(describe("0.4.0", "v0.4.0"), "0.4.0");
    }

    #[test]
    fn a_build_without_git_is_just_its_version() {
        assert_eq!(describe("0.4.0", ""), "0.4.0");
    }

    #[test]
    fn a_build_between_releases_names_its_commit() {
        assert_eq!(
            describe("0.4.0", "v0.4.0-3-g842efb4"),
            "0.4.0 (v0.4.0-3-g842efb4)"
        );
        assert_eq!(describe("0.4.0", "842efb4"), "0.4.0 (842efb4)");
    }

    #[test]
    fn this_build_names_the_cargo_version() {
        assert!(version_line().starts_with(VERSION), "{}", version_line());
    }
}
