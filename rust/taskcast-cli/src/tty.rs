//! TTY interaction utilities.
//!
//! This module contains interactive terminal prompts that require a real TTY
//! and cannot be tested in automated test suites. It is excluded from coverage.

use std::io::{IsTerminal, Write};

/// Prompt the user for confirmation. Returns Ok(true) if confirmed, Ok(false) if declined.
/// Returns Err if no TTY is available.
pub fn confirm_prompt(message: &str) -> Result<bool, Box<dyn std::error::Error>> {
    if !std::io::stdin().is_terminal() {
        return Err("[taskcast] No TTY detected. Re-run with --yes (-y) to skip confirmation.".into());
    }
    eprint!("{message}");
    std::io::stderr().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let trimmed = input.trim().to_lowercase();
    Ok(trimmed.is_empty() || trimmed == "y" || trimmed == "yes")
}
