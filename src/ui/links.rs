//! Reapply OSC 8 web links after ratatui's cell diff. Agent snapshots currently
//! carry text and styles, so only visible URLs can be recovered here.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::sync::OnceLock;

use crossterm::cursor::{RestorePosition, SavePosition};
use crossterm::queue;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use regex::Regex;
use unicode_width::UnicodeWidthStr;

type Point = (u16, u16);

#[derive(Default)]
pub(super) struct Links {
    previous: BTreeSet<Point>,
    last_frame: Option<Buffer>,
    last_area: Option<Rect>,
}

impl Links {
    /// Work from the final rendered buffer, including clipping and overlays.
    /// Repaint current links even when their text didn't change: ratatui can
    /// redraw any cell without knowing that its OSC 8 attribute needs restoring.
    pub fn prepare(&mut self, buffer: &Buffer, area: Option<Rect>) -> Vec<Run> {
        // An unchanged frame produces no output, so native selection/hover is
        // undisturbed and idle agents do not repaint links 60 times a second.
        if self.last_frame.as_ref() == Some(buffer) && self.last_area == area {
            return Vec::new();
        }
        self.last_frame = Some(buffer.clone());
        self.last_area = area;
        let links = area.map(|a| find_links(buffer, a)).unwrap_or_default();
        let points: BTreeSet<_> = self.previous.iter().chain(links.keys()).copied().collect();
        let mut runs: Vec<Run> = Vec::new();
        // Old link positions may now be the continuation half of a wide glyph.
        // Painting a space there would erase the glyph in the new frame.
        let mut starts = BTreeSet::new();
        for y in buffer.area.y..buffer.area.bottom() {
            let mut x = buffer.area.x;
            while x < buffer.area.right() {
                starts.insert((x, y));
                x = x.saturating_add(UnicodeWidthStr::width(buffer[(x, y)].symbol()).max(1) as u16);
            }
        }
        for point in points {
            if !starts.contains(&point) {
                continue;
            }
            let Some(cell) = buffer.cell(point) else {
                continue;
            };
            let url = links.get(&point).cloned();
            if let Some(run) = runs.last_mut().filter(|run| run.url == url) {
                run.cells.push((point, cell.clone()));
            } else {
                runs.push(Run {
                    url,
                    cells: vec![(point, cell.clone())],
                });
            }
        }
        self.previous = links.into_keys().collect();
        runs
    }
}

pub(super) struct Run {
    url: Option<String>,
    cells: Vec<(Point, Cell)>,
}

/// The terminal (including a browser terminal) opens the link on the user's
/// gesture. Never launch a browser on the daemon/container host.
pub(super) fn paint<W: Write>(writer: &mut W, runs: &[Run]) -> io::Result<()> {
    if runs.is_empty() {
        return Ok(());
    }
    queue!(writer, SavePosition)?;
    for run in runs {
        // URLs contain neither controls nor whitespace. The only escape bytes
        // emitted here are ours, never those from an agent's screen.
        write!(writer, "\x1b]8;;{}\x1b\\", run.url.as_deref().unwrap_or(""))?;
        CrosstermBackend::new(&mut *writer)
            .draw(run.cells.iter().map(|((x, y), cell)| (*x, *y, cell)))?;
        writer.write_all(b"\x1b]8;;\x1b\\")?;
    }
    queue!(writer, RestorePosition)?;
    writer.flush()
}

fn find_links(buffer: &Buffer, area: Rect) -> BTreeMap<Point, String> {
    static URL: OnceLock<Regex> = OnceLock::new();
    let re = URL.get_or_init(|| Regex::new(r#"https?://[^\s\p{Cc}<>"'`]+"#).unwrap());
    let area = area.intersection(buffer.area);
    let mut links = BTreeMap::new();
    for y in area.y..area.bottom() {
        let mut row = String::new();
        let mut cells = Vec::new();
        let mut x = area.x;
        while x < area.right() {
            let symbol = buffer[(x, y)].symbol();
            let width = UnicodeWidthStr::width(symbol).max(1) as u16;
            if x.saturating_add(width) > area.right() {
                break;
            }
            cells.push((row.len(), (x, y)));
            row.push_str(symbol);
            x += width;
        }
        for found in re.find_iter(&row) {
            let url = trim_punctuation(found.as_str());
            // Don't make a scheme with no host clickable.
            let host = url
                .split_once("://")
                .unwrap()
                .1
                .split(['/', '?', '#'])
                .next()
                .unwrap();
            if host.is_empty() {
                continue;
            }
            let end = found.start() + url.len();
            for &(offset, point) in &cells {
                if offset >= found.start() && offset < end {
                    links.insert(point, url.to_owned());
                }
            }
        }
    }
    links
}

fn trim_punctuation(mut url: &str) -> &str {
    loop {
        let previous = url;
        url = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '。', '，', '；', '！', '？']);
        for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
            if url.ends_with(close) && url.matches(close).count() > url.matches(open).count() {
                url = url.strip_suffix(close).unwrap();
            }
        }
        if url == previous {
            return url;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Style};

    #[test]
    fn detects_urls_across_styles_after_wide_text_and_trims_markdown() {
        let mut b = Buffer::empty(Rect::new(0, 0, 100, 2));
        b.set_string(
            0,
            0,
            "中文 [docs](https://example.test/a_(b)).",
            Style::default(),
        );
        b[(13, 0)].set_style(Style::default().fg(Color::Red));
        let links = find_links(&b, b.area);
        assert_eq!(links.get(&(13, 0)).unwrap(), "https://example.test/a_(b)");
        assert!(!links.contains_key(&(38, 0)));
        assert!(links.values().all(|u| u == "https://example.test/a_(b)"));
    }

    #[test]
    fn keeps_query_parameters_and_localhost_ports() {
        let b = Buffer::with_lines(["http://localhost:3000/?a=1&b=two#section"]);
        let links = find_links(&b, b.area);
        assert!(links
            .values()
            .all(|u| u == "http://localhost:3000/?a=1&b=two#section"));
        assert!(!links.is_empty());
    }

    #[test]
    fn ignores_other_schemes_and_control_characters() {
        let b = Buffer::with_lines(["file:///tmp/a javascript:alert(1) https://"]);
        assert!(find_links(&b, b.area).is_empty());
        let b = Buffer::with_lines(["https://example.test/\x1b]52;bad"]);
        for url in find_links(&b, b.area).values() {
            assert!(!url.chars().any(char::is_control));
        }
    }

    #[test]
    fn repaint_changes_the_target_even_for_an_unchanged_prefix() {
        let mut state = Links::default();
        let a = Buffer::with_lines(["https://example.test/one"]);
        state.prepare(&a, Some(a.area));
        let b = Buffer::with_lines(["https://example.test/two"]);
        let runs = state.prepare(&b, Some(b.area));
        assert_eq!(runs[0].url.as_deref(), Some("https://example.test/two"));
        assert_eq!(runs[0].cells[0].0, (0, 0));
    }

    #[test]
    fn leaving_the_session_clears_links_without_erasing_new_text() {
        let mut state = Links::default();
        let a = Buffer::with_lines(["https://example.test"]);
        state.prepare(&a, Some(a.area));
        let b = Buffer::with_lines(["back at the board  "]);
        let runs = state.prepare(&b, None);
        assert!(runs.iter().all(|r| r.url.is_none()));
        assert_eq!(runs[0].cells[0].1.symbol(), "b");
        assert!(state.prepare(&b, None).is_empty());
    }

    #[test]
    fn identical_frames_do_not_write_over_native_selection() {
        let mut state = Links::default();
        let b = Buffer::with_lines(["https://example.test"]);
        assert!(!state.prepare(&b, Some(b.area)).is_empty());
        assert!(state.prepare(&b, Some(b.area)).is_empty());
    }

    #[test]
    fn clearing_a_link_does_not_paint_over_a_wide_continuation() {
        let mut state = Links::default();
        let a = Buffer::with_lines(["https://example.test"]);
        state.prepare(&a, Some(a.area));
        let mut b = Buffer::empty(a.area);
        b.set_string(0, 0, "中文", Style::default());
        let runs = state.prepare(&b, None);
        assert!(runs
            .iter()
            .flat_map(|r| &r.cells)
            .all(|(p, _)| p.0 != 1 && p.0 != 3));
    }

    #[test]
    fn emitted_links_close_and_preserve_text_styles_and_cursor() {
        let mut b = Buffer::with_lines(["https://example.test"]);
        b[(0, 0)].set_style(Style::default().fg(Color::Red));
        let runs = Links::default().prepare(&b, Some(b.area));
        let mut bytes = Vec::new();
        paint(&mut bytes, &runs).unwrap();
        let output = String::from_utf8(bytes).unwrap();
        assert!(output.contains("\x1b]8;;https://example.test\x1b\\"));
        assert!(output.contains("\x1b]8;;\x1b\\"));
        assert!(output.ends_with("\x1b8"));
        let mut terminal = vt100::Parser::new(2, 60, 0);
        terminal.process(b"\x1b[2;5H");
        terminal.process(output.as_bytes());
        assert_eq!(terminal.screen().cursor_position(), (1, 4));
        assert_eq!(terminal.screen().contents(), "https://example.test");
        // Crossterm honors NO_COLOR in the test environment; check that its
        // input still carries the original style rather than forcing colors.
        assert_eq!(runs[0].cells[0].1.fg, Color::Red);
    }

    #[test]
    fn only_the_visible_agent_area_is_linked() {
        let b = Buffer::with_lines([
            "https://title.test",
            "https://agent.test",
            "https://footer.test",
        ]);
        let links = find_links(&b, Rect::new(0, 1, 18, 1));
        assert!(links.keys().all(|(_, y)| *y == 1));
        assert_eq!(links.get(&(0, 1)).unwrap(), "https://agent.test");
    }
}
