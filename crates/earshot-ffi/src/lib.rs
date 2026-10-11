//! C ABI over the Earshot acoustics engine, called from Java through the FFM
//! API.
//!
//! This is the only crate allowed to use `unsafe`. Run `cargo xtask header`
//! after changing any export to regenerate `include/earshot.h`.
//!
//! For now (M0) it only proves the native library builds, ships in the jar and
//! loads; the world mirror and the simulation arrive in M1 (docs/PLAN.md).

#![allow(unsafe_code)]

/// Version of this C ABI. Java refuses to use a library whose version differs
/// from the one it was built against. Bump on any incompatible change.
pub const EARSHOT_ABI_VERSION: u32 = 1;

/// Returns [`EARSHOT_ABI_VERSION`]. Safe to call before anything else.
#[unsafe(no_mangle)]
pub extern "C" fn earshot_abi_version() -> u32 {
    EARSHOT_ABI_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abi_version_matches_constant() {
        assert_eq!(earshot_abi_version(), EARSHOT_ABI_VERSION);
    }

    /// `cargo xtask header --check` keeps the header current, but only when
    /// it runs: this catches a function added without regenerating it.
    #[test]
    fn header_declares_every_exported_function() {
        let crate_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut exported = std::collections::BTreeSet::new();
        for entry in std::fs::read_dir(crate_dir.join("src")).unwrap() {
            let source = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            for after in source.split("extern \"C\" fn ").skip(1) {
                exported.insert(identifier(after));
            }
        }
        let header = std::fs::read_to_string(crate_dir.join("include/earshot.h")).unwrap();
        let declared: std::collections::BTreeSet<String> = header
            .match_indices("earshot_")
            .map(|(at, _)| identifier(&header[at..]))
            .filter(|name| header.contains(&format!("{name}(")))
            .collect();
        assert!(!exported.is_empty());
        assert_eq!(exported, declared, "run `cargo xtask header`");
    }

    /// The identifier `text` starts with.
    fn identifier(text: &str) -> String {
        text.chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect()
    }
}
