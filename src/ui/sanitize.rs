/// Replace terminal control characters in untrusted display text.
#[must_use]
pub(crate) fn terminal_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                '\u{fffd}'
            } else {
                character
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_controls_are_replaced_without_changing_printable_text() {
        assert_eq!(
            terminal_text("deploy\n\u{1b}]52;clipboard\u{7} café"),
            "deploy��]52;clipboard� café"
        );
    }
}
