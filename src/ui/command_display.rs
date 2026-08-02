use std::ffi::{OsStr, OsString};

use crate::candidate::Candidate;

#[derive(Debug, thiserror::Error)]
pub enum DisplayError {
    #[error("cannot render non-Unicode value as a reproducible shell command: {0}")]
    NonUnicode(String),
    #[error("cannot safely print a shell command containing terminal control characters")]
    ControlCharacters,
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
    let value = value
        .to_str()
        .ok_or_else(|| DisplayError::NonUnicode(diagnostic_value(value)))?;
    if value.chars().any(char::is_control) {
        return Err(DisplayError::ControlCharacters);
    }
    Ok(value)
}

fn diagnostic_value(value: &OsStr) -> String {
    if let Some(value) = value.to_str() {
        return format!("{value:?}");
    }
    diagnostic_opaque(value)
}

#[cfg(unix)]
fn diagnostic_opaque(value: &OsStr) -> String {
    use std::fmt::Write as _;
    use std::os::unix::ffi::OsStrExt as _;

    let mut output = String::from("b\"");
    for byte in value.as_bytes() {
        match byte {
            b'\\' => output.push_str("\\\\"),
            b'\"' => output.push_str("\\\""),
            b'\n' => output.push_str("\\n"),
            b'\r' => output.push_str("\\r"),
            b'\t' => output.push_str("\\t"),
            0x20..=0x7e => output.push(char::from(*byte)),
            _ => write!(output, "\\x{byte:02x}").expect("writing to a String cannot fail"),
        }
    }
    output.push('"');
    output
}

#[cfg(windows)]
fn diagnostic_opaque(value: &OsStr) -> String {
    use std::fmt::Write as _;
    use std::os::windows::ffi::OsStrExt as _;

    let mut output = String::from("w\"");
    for unit in value.encode_wide() {
        write!(output, "\\u{{{unit:04x}}}").expect("writing to a String cannot fail");
    }
    output.push('"');
    output
}

#[cfg(not(any(unix, windows)))]
fn diagnostic_opaque(value: &OsStr) -> String {
    format!("{:?}", value.to_string_lossy())
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
    use crate::registry::{NODE, NODE_SOURCE};

    use super::*;

    #[test]
    fn posix_rendering_quotes_empty_and_single_quote_arguments() -> anyhow::Result<()> {
        let candidate = Candidate::new(
            "test",
            NODE,
            NODE_SOURCE,
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

    #[test]
    fn powershell_rendering_quotes_cwd_environment_and_arguments() -> anyhow::Result<()> {
        let mut candidate = Candidate::new(
            "test",
            NODE,
            NODE_SOURCE,
            Intent::Run,
            "test",
            "tool.exe",
            vec![OsString::from("it's"), OsString::new()],
            PathBuf::from("C:/work/a b"),
            1,
            SelectionPolicy::Automatic,
        );
        candidate
            .env
            .insert(OsString::from("MODE"), OsString::from("reader's"));
        let rendered = powershell(&candidate, &[OsString::from("tail arg")])?;
        assert_eq!(
            rendered,
            "Set-Location -LiteralPath 'C:/work/a b'; $env:MODE = 'reader''s'; & 'tool.exe' 'it''s' '' 'tail arg'"
        );
        Ok(())
    }

    #[test]
    fn copyable_commands_reject_terminal_controls() {
        let candidate = Candidate::new(
            "test",
            NODE,
            NODE_SOURCE,
            Intent::Run,
            "test",
            "tool",
            vec![OsString::from("unsafe\u{1b}[2J")],
            PathBuf::from("/tmp"),
            1,
            SelectionPolicy::Automatic,
        );
        assert!(matches!(
            posix(&candidate, &[]),
            Err(DisplayError::ControlCharacters)
        ));
        assert!(matches!(
            powershell(&candidate, &[]),
            Err(DisplayError::ControlCharacters)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn diagnostic_rendering_preserves_non_utf8_bytes_and_controls() {
        use std::os::unix::ffi::OsStringExt as _;

        let mut candidate = Candidate::new(
            "test",
            NODE,
            NODE_SOURCE,
            Intent::Run,
            "test",
            OsString::from_vec(vec![b't', 0x80, b'o', b'o', b'l']),
            vec![OsString::from_vec(vec![b'a', b'\n', 0xff])],
            PathBuf::from("/tmp"),
            1,
            SelectionPolicy::Automatic,
        );
        candidate.passthrough = crate::candidate::PassthroughStyle::Append;
        assert_eq!(diagnostic(&candidate, &[]), r#"b"t\x80ool" b"a\n\xff""#);
    }
}
