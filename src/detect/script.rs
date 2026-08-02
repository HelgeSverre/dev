use std::ffi::OsString;
use std::io::Read as _;
use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Shebang {
    pub program: OsString,
    pub arguments: Vec<OsString>,
    pub display: String,
}

pub(super) fn read_shebang(path: &Path) -> Option<Shebang> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut bytes = [0_u8; 512];
    let read = file.read(&mut bytes).ok()?;
    let first_line = bytes[..read].split(|byte| *byte == b'\n').next()?;
    let text = std::str::from_utf8(first_line).ok()?.trim_end_matches('\r');
    let command = text.strip_prefix("#!")?.trim();
    if command.contains(['\'', '"', '\\']) {
        return None;
    }
    let mut words = command.split_ascii_whitespace();
    let declared = words.next()?;
    let declared_name = Path::new(declared).file_name()?.to_str()?;

    let (program, arguments) = if declared_name == "env" {
        let mut remaining = words.collect::<Vec<_>>();
        if remaining.first().copied() == Some("-S") {
            remaining.remove(0);
        }
        let interpreter = remaining.first().copied()?;
        if !recognized_interpreter(interpreter) {
            return None;
        }
        (
            OsString::from(interpreter),
            remaining[1..].iter().map(OsString::from).collect(),
        )
    } else {
        if !recognized_interpreter(declared_name) {
            return None;
        }
        (
            OsString::from(declared),
            words.map(OsString::from).collect(),
        )
    };
    Some(Shebang {
        program,
        arguments,
        display: command.to_owned(),
    })
}

fn recognized_interpreter(program: &str) -> bool {
    matches!(
        program,
        "sh" | "bash"
            | "dash"
            | "ksh"
            | "zsh"
            | "fish"
            | "python"
            | "python3"
            | "ruby"
            | "php"
            | "node"
            | "deno"
            | "perl"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_split_shebang_preserves_interpreter_arguments() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let script = temp.path().join("probe");
        std::fs::write(&script, "#!/usr/bin/env -S python3 -u\nprint('ok')\n")?;

        let shebang = read_shebang(&script).ok_or_else(|| anyhow::anyhow!("missing shebang"))?;
        assert_eq!(shebang.program, "python3");
        assert_eq!(shebang.arguments, ["-u"]);
        Ok(())
    }

    #[test]
    fn unknown_shebang_is_not_guessed() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let script = temp.path().join("probe");
        std::fs::write(&script, "#!/opt/custom/runtime\n")?;
        assert_eq!(read_shebang(&script), None);
        Ok(())
    }

    #[test]
    fn quoted_env_split_shebang_is_not_misparsed() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let script = temp.path().join("probe");
        std::fs::write(&script, "#!/usr/bin/env -S 'python3 -u'\n")?;
        assert_eq!(read_shebang(&script), None);
        Ok(())
    }
}
