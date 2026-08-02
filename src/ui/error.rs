use std::fmt::Write;

use crate::resolve::Resolution;

use super::command_display;

#[must_use]
pub fn candidate_table(resolution: &Resolution) -> String {
    let mut output = String::new();
    let _ = writeln!(
        output,
        "dev: resolution requires an interactive choice ({:?})",
        resolution.reason
    );
    for ranked in &resolution.candidates {
        let _ = writeln!(
            output,
            "  {:>4}  {}",
            ranked.candidate.structural_points + ranked.query.total_points,
            command_display::diagnostic(&ranked.candidate, &[])
        );
    }
    output
}
