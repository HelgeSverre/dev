use std::ffi::{OsStr, OsString};

use crate::candidate::Candidate;

#[derive(Debug, thiserror::Error)]
pub enum DisplayError {
    #[error("cannot render non-Unicode value as a reproducible shell command: {0}")]
    NonUnicode(String),
}

/// Render the exact argv as an inspectable, non-shell diagnostic string.
#[must_use]
pub fn diagnostic(candidate: &Candidate, passthrough: &[OsString]) -> String {
    std::iter::once(&candidate.program)
        .chain(candidate.command_with_passthrough(passthrough).iter())
        .map(|value| diagnostic_value(value))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Render a reproducible POSIX-shell command including cwd and environment deltas.
pub fn posix(candidate: &Candidate, passthrough: &[OsString]) -> Result<String, DisplayError> {
    let cwd = unicode(candidate.cwd.as_os_str())?;
    let mut output = format!("cd {} && ", posix_quote(cwd));
    if !candidate.env.is_empty() {
        output.push_str("env ");
        for (key, value) in &candidate.env {
            output.push_str(&posix_quote(unicode(key)?));
            output.push('=');
            output.push_str(&posix_quote(unicode(value)?));
            output.push(' ');
        }
    }
    output.push_str(&posix_quote(unicode(&candidate.program)?));
    for argument in candidate.command_with_passthrough(passthrough) {
        output.push(' ');
        output.push_str(&posix_quote(unicode(&argument)?));
    }
    Ok(output)
}

/// Render a reproducible PowerShell command including cwd and environment deltas.
pub fn powershell(candidate: &Candidate, passthrough: &[OsString]) -> Result<String, DisplayError> {
    let mut statements = vec![format!(
        "Set-Location -LiteralPath {}",
        powershell_quote(unicode(candidate.cwd.as_os_str())?)
    )];
    for (key, value) in &candidate.env {
        statements.push(format!(
            "$env:{} = {}",
            unicode(key)?,
            powershell_quote(unicode(value)?)
        ));
    }
    let mut command = format!("& {}", powershell_quote(unicode(&candidate.program)?));
    for argument in candidate.command_with_passthrough(passthrough) {
        command.push(' ');
        command.push_str(&powershell_quote(unicode(&argument)?));
    }
    statements.push(command);
    Ok(statements.join("; "))
}

fn unicode(value: &OsStr) -> Result<&str, DisplayError> {
    value
        .to_str()
        .ok_or_else(|| DisplayError::NonUnicode(diagnostic_value(value)))
}

fn diagnostic_value(value: &OsStr) -> String {
    let display = value.to_string_lossy();
    format!("{:?}", display.as_ref())
}

fn posix_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&byte))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::candidate::{Candidate, SelectionPolicy};
    use crate::intent::Intent;

    use super::*;

    #[test]
    fn posix_rendering_quotes_empty_and_single_quote_arguments() -> anyhow::Result<()> {
        let candidate = Candidate::new(
            "test",
            "test",
            Intent::Run,
            "test",
            "tool",
            vec![OsString::from("it's"), OsString::new()],
            PathBuf::from("/tmp/a b"),
            1,
            SelectionPolicy::Automatic,
        );
        let rendered = posix(&candidate, &[])?;
        assert_eq!(rendered, "cd '/tmp/a b' && tool 'it'\"'\"'s' ''");
        Ok(())
    }
}
