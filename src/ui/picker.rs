use std::io::{self, IsTerminal, Stderr};
use std::time::Duration;

use ratatui::crossterm::cursor::{Hide, Show};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::candidate::Availability;
use crate::query::rank::compare_hinted;
use crate::query::{match_candidate, normalize_query, QueryMatch};
use crate::resolve::Resolution;

use super::command_display;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum PickerOutcome {
    Run { index: usize, remember: bool },
    Print { index: usize },
    Cancel,
}

#[derive(Debug, thiserror::Error)]
pub enum PickerError {
    #[error("picker requires interactive stdin and stderr")]
    NotInteractive,
    #[error("terminal error: {0}")]
    Terminal(#[from] io::Error),
}

#[derive(Debug)]
struct PickerApp<'a> {
    resolution: &'a Resolution,
    query: String,
    chaos: u8,
    visible: Vec<(usize, QueryMatch)>,
    state: ListState,
    show_details: bool,
    colors: bool,
}

impl<'a> PickerApp<'a> {
    fn new(resolution: &'a Resolution, hints: &[String], chaos: u8, colors: bool) -> Self {
        let mut app = Self {
            resolution,
            query: if resolution.status == crate::resolve::ResolutionStatus::HintNoMatch {
                String::new()
            } else {
                hints.join(" ")
            },
            chaos,
            visible: Vec::new(),
            state: ListState::default(),
            show_details: true,
            colors,
        };
        app.refresh();
        app
    }

    fn refresh(&mut self) {
        let hints = self
            .query
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let query = normalize_query(&hints);
        self.visible = self
            .resolution
            .candidates
            .iter()
            .enumerate()
            .filter_map(|(index, ranked)| {
                let matched = match_candidate(&ranked.candidate, &query, self.chaos);
                (query.is_empty() || matched.matched_meaningful_terms > 0)
                    .then_some((index, matched))
            })
            .collect();
        if !query.is_empty() {
            self.visible.sort_by(|left, right| {
                compare_hinted(
                    (&self.resolution.candidates[left.0].candidate, &left.1),
                    (&self.resolution.candidates[right.0].candidate, &right.1),
                )
            });
        }
        self.state.select((!self.visible.is_empty()).then_some(0));
    }

    fn selected_index(&self) -> Option<usize> {
        self.state
            .selected()
            .and_then(|selected| self.visible.get(selected))
            .map(|(index, _)| *index)
    }

    fn select_next(&mut self) {
        if self.visible.is_empty() {
            self.state.select(None);
            return;
        }
        let next = self
            .state
            .selected()
            .map_or(0, |selected| (selected + 1) % self.visible.len());
        self.state.select(Some(next));
    }

    fn select_previous(&mut self) {
        if self.visible.is_empty() {
            self.state.select(None);
            return;
        }
        let previous = self.state.selected().map_or(0, |selected| {
            selected
                .checked_sub(1)
                .unwrap_or_else(|| self.visible.len() - 1)
        });
        self.state.select(Some(previous));
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<PickerOutcome> {
        if key.kind != KeyEventKind::Press {
            return None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return match key.code {
                KeyCode::Char('c') => Some(PickerOutcome::Cancel),
                KeyCode::Char('r') => self.selected_index().map(|index| PickerOutcome::Run {
                    index,
                    remember: true,
                }),
                KeyCode::Char('d') => self
                    .selected_index()
                    .map(|index| PickerOutcome::Print { index }),
                _ => None,
            };
        }
        match key.code {
            KeyCode::Esc => Some(PickerOutcome::Cancel),
            KeyCode::Enter => self.selected_index().map(|index| PickerOutcome::Run {
                index,
                remember: false,
            }),
            KeyCode::Down => {
                self.select_next();
                None
            }
            KeyCode::Up => {
                self.select_previous();
                None
            }
            KeyCode::Tab => {
                self.show_details = !self.show_details;
                None
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.refresh();
                None
            }
            KeyCode::Char(character) => {
                self.query.push(character);
                self.refresh();
                None
            }
            _ => None,
        }
    }
}

type PickerTerminal = Terminal<ratatui::backend::CrosstermBackend<Stderr>>;

struct TerminalSession {
    terminal: PickerTerminal,
}

impl TerminalSession {
    fn start() -> Result<Self, io::Error> {
        enable_raw_mode()?;
        let mut stderr = io::stderr();
        if let Err(error) = execute!(stderr, EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        let backend = ratatui::backend::CrosstermBackend::new(stderr);
        match Terminal::new(backend) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                let _ = disable_raw_mode();
                let mut stderr = io::stderr();
                let _ = execute!(stderr, LeaveAlternateScreen, Show);
                Err(error)
            }
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen, Show);
        let _ = self.terminal.show_cursor();
    }
}

/// Open the interactive candidate picker.
pub fn pick(
    resolution: &Resolution,
    hints: &[String],
    chaos: u8,
    colors: bool,
) -> Result<PickerOutcome, PickerError> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err(PickerError::NotInteractive);
    }
    let mut session = TerminalSession::start()?;
    let mut app = PickerApp::new(resolution, hints, chaos, colors);
    loop {
        session.terminal.draw(|frame| render(frame, &mut app))?;
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        if let Event::Key(key) = event::read()? {
            if let Some(outcome) = app.handle_key(key) {
                return Ok(outcome);
            }
        }
    }
}

fn render(frame: &mut Frame<'_>, app: &mut PickerApp<'_>) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(4),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let title = match app.resolution.status {
        crate::resolve::ResolutionStatus::HintNoMatch => "no hints matched — choose a candidate",
        _ => "choose a project command",
    };
    frame.render_widget(
        Paragraph::new(app.query.as_str()).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" dev — {title} ")),
        ),
        header,
    );

    if body.width >= 80 && app.show_details {
        let [list, details] =
            Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
                .areas(body);
        render_list(frame, app, list);
        render_details(frame, app, details);
    } else {
        let [list, details] = Layout::vertical([
            Constraint::Percentage(if app.show_details { 60 } else { 100 }),
            Constraint::Percentage(if app.show_details { 40 } else { 0 }),
        ])
        .areas(body);
        render_list(frame, app, list);
        if app.show_details {
            render_details(frame, app, details);
        }
    }
    frame.render_widget(
        Paragraph::new("↵ run  ^R run+remember  ^D print  tab details  esc cancel").style(
            color_style(app.colors, Style::default().fg(Color::DarkGray)),
        ),
        footer,
    );
}

fn render_list(frame: &mut Frame<'_>, app: &mut PickerApp<'_>, area: Rect) {
    let items = app
        .visible
        .iter()
        .map(|(index, matched)| {
            let candidate = &app.resolution.candidates[*index].candidate;
            let available = candidate.availability.is_available();
            let warning = if available { "" } else { "⚠ " };
            let score = candidate.structural_points + matched.total_points;
            let style = if !app.colors || available {
                Style::default()
            } else {
                Style::default().fg(Color::DarkGray)
            };
            ListItem::new(vec![
                Line::from(vec![
                    Span::raw(warning),
                    Span::styled(
                        &candidate.label,
                        color_style(app.colors, Style::default().add_modifier(Modifier::BOLD)),
                    ),
                    Span::raw(format!("  {score}")),
                ]),
                Line::styled(
                    &candidate.description,
                    color_style(app.colors, Style::default().fg(Color::DarkGray)),
                ),
            ])
            .style(style)
        })
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(format!(
            " {} candidate{} ",
            app.visible.len(),
            if app.visible.len() == 1 { "" } else { "s" }
        )))
        .highlight_symbol("● ")
        .highlight_style(color_style(
            app.colors,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    frame.render_stateful_widget(list, area, &mut app.state);
}

fn render_details(frame: &mut Frame<'_>, app: &PickerApp<'_>, area: Rect) {
    let Some(selected) = app.state.selected() else {
        frame.render_widget(
            Paragraph::new("No candidate matches the current query")
                .block(Block::default().borders(Borders::ALL).title(" Details ")),
            area,
        );
        return;
    };
    let Some((index, query)) = app.visible.get(selected) else {
        return;
    };
    let candidate = &app.resolution.candidates[*index].candidate;
    let mut lines = vec![
        Line::styled(
            command_display::diagnostic(candidate, &[]),
            color_style(app.colors, Style::default().fg(Color::Cyan)),
        ),
        Line::from(format!("cwd: {}", candidate.cwd.display())),
        Line::from(format!("availability: {:?}", candidate.availability)),
        Line::from(""),
    ];
    if !query.terms.is_empty() {
        lines.push(Line::styled(
            format!(
                "Query match — coverage {}/{}",
                query.matched_meaningful_terms, query.meaningful_terms
            ),
            color_style(app.colors, Style::default().add_modifier(Modifier::BOLD)),
        ));
        lines.extend(query.terms.iter().map(|matched| {
            Line::from(format!(
                "{:+4} {:?}: {} → {}",
                matched.points, matched.class, matched.hint, matched.candidate_value
            ))
        }));
        lines.push(Line::from(""));
    }
    lines.push(Line::styled(
        "Structural evidence",
        color_style(app.colors, Style::default().add_modifier(Modifier::BOLD)),
    ));
    lines.extend(
        candidate
            .evidence
            .iter()
            .map(|evidence| Line::from(format!("{:+4} {}", evidence.points, evidence.reason))),
    );
    let availability_style = if !app.colors {
        Style::default()
    } else {
        match candidate.availability {
            Availability::Available { .. } => Style::default(),
            Availability::MissingProgram { .. } | Availability::UnsupportedHost { .. } => {
                Style::default().fg(Color::Yellow)
            }
        }
    };
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .style(availability_style)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(" Details ")),
        area,
    );
}

fn color_style(enabled: bool, style: Style) -> Style {
    if enabled {
        style
    } else {
        Style::default()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::candidate::{Availability, Candidate, SearchDocument, SelectionPolicy};
    use crate::intent::Intent;
    use crate::resolve::{RankedCandidate, ResolutionReason, ResolutionStatus};

    use super::*;

    fn resolution() -> Resolution {
        let candidates = ["alpha", "beta"]
            .into_iter()
            .map(|name| {
                let mut candidate = Candidate::new(
                    name,
                    "node",
                    Intent::Run,
                    name,
                    "true",
                    Vec::new(),
                    PathBuf::from("/tmp"),
                    50,
                    SelectionPolicy::Automatic,
                );
                candidate.label = name.to_owned();
                candidate.search = SearchDocument {
                    identities: vec![name.to_owned()],
                    ..SearchDocument::default()
                };
                candidate.availability = Availability::Available {
                    resolved_program: PathBuf::from("/usr/bin/true"),
                };
                RankedCandidate {
                    candidate,
                    query: QueryMatch::default(),
                    finalist: false,
                }
            })
            .collect();
        Resolution {
            status: ResolutionStatus::Ambiguous,
            reason: ResolutionReason::CloseCandidates,
            selected: None,
            candidates,
        }
    }

    #[test]
    fn interactive_filter_uses_shared_matcher_and_clear_reveals_all() {
        let resolution = resolution();
        let mut app = PickerApp::new(&resolution, &["beta".to_owned()], 1, false);
        assert_eq!(app.visible.len(), 1);
        assert_eq!(app.selected_index(), Some(1));
        app.query.clear();
        app.refresh();
        assert_eq!(app.visible.len(), 2);
    }

    #[test]
    fn control_actions_return_the_selected_candidate() {
        let resolution = resolution();
        let mut app = PickerApp::new(&resolution, &[], 1, false);
        let remembered = app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert_eq!(
            remembered,
            Some(PickerOutcome::Run {
                index: 0,
                remember: true
            })
        );
        let once = app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            once,
            Some(PickerOutcome::Run {
                index: 0,
                remember: false
            })
        );
        let cancelled = app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(cancelled, Some(PickerOutcome::Cancel));
    }

    #[test]
    fn unmatched_cli_hints_open_an_unfiltered_picker() {
        let mut resolution = resolution();
        resolution.status = ResolutionStatus::HintNoMatch;
        resolution.reason = ResolutionReason::HintNoMatch;
        let app = PickerApp::new(&resolution, &["purple-monkey".to_owned()], 1, false);
        assert!(app.query.is_empty());
        assert_eq!(app.visible.len(), 2);
    }
}
