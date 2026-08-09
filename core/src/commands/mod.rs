//! One module per group of subcommands, each holding what used to sit in the
//! CLI's `main.rs`.
//!
//! These still print — a `ps` here writes the same table it always did — so the
//! CLI is a thin pass-through. The daemon does not call them; it goes through
//! [`crate::api`], which returns the same information as data.

pub mod boot;
pub mod flavor;
pub mod guest;
pub mod images;
pub mod machines;
pub mod probe;
#[cfg(target_os = "macos")]
pub mod store;
pub mod volumes;

/// Truncate a string to `n` display chars, adding an ellipsis if cut.
///
/// Shared by every table-printing subcommand, which is why it lives here rather
/// than beside any one of them.
pub(crate) fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let head: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{head}…")
    }
}
