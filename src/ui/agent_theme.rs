//! Follow saved agent appearance without rewriting the agent's preferences.
use crate::theme::Theme;
use std::path::PathBuf;
use std::time::Instant;

thread_local! {
    static ACTIVE: std::cell::Cell<Option<Theme>> = const { std::cell::Cell::new(None) };
}

pub(super) fn active() -> Option<Theme> {
    ACTIVE.with(|v| v.get())
}

pub(super) struct Scope(Option<Theme>);
impl Scope {
    pub fn new(theme: Option<Theme>) -> Self {
        Self(ACTIVE.with(|v| v.replace(theme)))
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        ACTIVE.with(|v| v.set(self.0));
    }
}

pub(super) struct Follower {
    claude: PathBuf,
    legacy_claude: PathBuf,
    codex: PathBuf,
    profiles: PathBuf,
    last: Option<(Option<String>, Instant, Option<Theme>)>,
}

impl Follower {
    pub fn new(socket: &std::path::Path) -> Self {
        let home = crate::sys::home().unwrap_or_default();
        let claude = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".claude"));
        let codex = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"));
        Self {
            claude: claude.join("settings.json"),
            legacy_claude: if std::env::var_os("CLAUDE_CONFIG_DIR").is_some() {
                claude.join(".claude.json")
            } else {
                home.join(".claude.json")
            },
            codex: codex.join("config.toml"),
            profiles: crate::profile::profiles_dir_for_socket(socket),
            last: None,
        }
    }

    pub fn poll(&mut self, profile: Option<&str>, now: Instant) -> Option<Theme> {
        if let Some((previous, checked, theme)) = &self.last {
            if previous.as_deref() == profile
                && now.saturating_duration_since(*checked).as_millis() < 500
            {
                return *theme;
            }
        }
        // Built-in aliases such as glm also run Claude. Unknown/custom profiles
        // keep the terminal's colors rather than guessing from their name.
        let command = profile
            .filter(|name| !self.has_override(name))
            .and_then(crate::profile::Profile::builtin)
            .and_then(|p| p.command.first().cloned());
        let theme = match command.as_deref() {
            Some("claude") => self.claude_theme(),
            Some("codex") => std::fs::read_to_string(&self.codex)
                .ok()
                .and_then(|s| s.parse::<toml::Value>().ok())
                .and_then(|v| v.get("tui")?.get("theme")?.as_str().and_then(codex_theme)),
            _ => None,
        };
        self.last = Some((profile.map(str::to_owned), now, theme));
        theme
    }

    fn has_override(&self, name: &str) -> bool {
        let entries = match std::fs::read_dir(&self.profiles) {
            Ok(entries) => entries,
            Err(e) => return e.kind() != std::io::ErrorKind::NotFound,
        };
        entries.filter_map(Result::ok).any(|entry| {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                return false;
            }
            std::fs::read_to_string(path)
                .ok()
                .and_then(|s| crate::profile::Profile::from_toml(&s).ok())
                .is_some_and(|p| p.name == name)
        })
    }

    fn claude_theme(&self) -> Option<Theme> {
        let read = |path: &PathBuf| -> Option<String> {
            let v: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
            v.get("theme")?.as_str().map(str::to_owned)
        };
        let name = read(&self.claude).or_else(|| read(&self.legacy_claude))?;
        match name.as_str() {
            "light" | "light-ansi" | "light-daltonized" => Some(Theme::Light),
            "dark" | "dark-ansi" | "dark-daltonized" => Some(Theme::Dark),
            _ => None,
        }
    }
}

// Codex's bundled names, from codex-rs/tui/src/render/highlight.rs.
// These select light/dark defaults, not replacements for syntax accent colors.
fn codex_theme(name: &str) -> Option<Theme> {
    match name {
        "base16-ocean-light"
        | "catppuccin-latte"
        | "coldark-cold"
        | "github"
        | "gruvbox-light"
        | "inspired-github"
        | "monokai-extended-light"
        | "one-half-light"
        | "solarized-light" => Some(Theme::Light),
        "base16-eighties-dark"
        | "base16-mocha-dark"
        | "base16-ocean-dark"
        | "catppuccin-frappe"
        | "catppuccin-macchiato"
        | "catppuccin-mocha"
        | "coldark-dark"
        | "dark-neon"
        | "dracula"
        | "gruvbox-dark"
        | "1337"
        | "monokai-extended"
        | "monokai-extended-bright"
        | "monokai-extended-origin"
        | "nord"
        | "one-half-dark"
        | "solarized-dark"
        | "sublime-snazzy"
        | "two-dark"
        | "zenburn" => Some(Theme::Dark),
        _ => None,
    }
}

pub(super) fn colors(theme: Theme) -> (ratatui::style::Color, ratatui::style::Color) {
    use ratatui::style::Color;
    match theme {
        Theme::Light => (Color::Rgb(32, 33, 36), Color::Rgb(255, 255, 255)),
        _ => (Color::Rgb(229, 229, 229), Color::Rgb(24, 24, 24)),
    }
}

/// OSC changes are only sent in the browser deployment. Native terminals keep
/// their own palette; dct paints matching default cells inside its alternate screen.
pub(super) fn browser_sequence(theme: Option<Theme>) -> &'static str {
    match theme {
        Some(Theme::Light) => "\x1b]10;#202124\x07\x1b]11;#ffffff\x07\x1b]12;#202124\x07",
        Some(Theme::Dark) => "\x1b]10;#e5e5e5\x07\x1b]11;#181818\x07\x1b]12;#e5e5e5\x07",
        _ => "\x1b]110\x07\x1b]111\x07\x1b]112\x07",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn an_overridden_builtin_profile_keeps_terminal_defaults() {
        let (_dir, mut follower) = fixture();
        std::fs::write(&follower.claude, r#"{"theme":"light"}"#).unwrap();
        std::fs::create_dir(&follower.profiles).unwrap();
        std::fs::write(
            follower.profiles.join("my-agent.toml"),
            "name = 'claude'\ncommand = ['another-agent']\n",
        )
        .unwrap();
        assert_eq!(follower.poll(Some("claude"), Instant::now()), None);
    }

    #[test]
    fn rendering_changes_default_cells_but_keeps_agent_accent_colors() {
        use crate::pty::{ScreenColor, ScreenSpan, ScreenStyle};
        use ratatui::{backend::TestBackend, style::Color, Terminal};
        let (mut app, _dir) = crate::ui::app::App::test_app();
        app.view = crate::ui::View::Attached(1);
        app.screen = vec![vec![
            ScreenSpan {
                text: "prompt".into(),
                style: ScreenStyle::default(),
            },
            ScreenSpan {
                text: "!".into(),
                style: ScreenStyle {
                    fg: ScreenColor::Rgb(210, 30, 50),
                    ..ScreenStyle::default()
                },
            },
        ]];
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|f| crate::ui::draw_with_theme(f, &mut app, Some(Theme::Light)))
            .unwrap();
        let (x, y) = app.screen_origin.unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(x, y)].fg, Color::Rgb(32, 33, 36));
        assert_eq!(buffer[(x, y)].bg, Color::Rgb(255, 255, 255));
        assert_eq!(buffer[(x + 6, y)].fg, Color::Rgb(210, 30, 50));
        assert_eq!(buffer[(79, 10)].bg, Color::Rgb(255, 255, 255));
        terminal
            .draw(|f| crate::ui::draw_with_theme(f, &mut app, Some(Theme::Dark)))
            .unwrap();
        assert_eq!(
            terminal.backend().buffer()[(x, y)].bg,
            Color::Rgb(24, 24, 24)
        );
        terminal
            .draw(|f| crate::ui::draw_with_theme(f, &mut app, None))
            .unwrap();
        assert_eq!(terminal.backend().buffer()[(x, y)].bg, Color::Reset);
    }

    fn fixture() -> (tempfile::TempDir, Follower) {
        let dir = tempfile::tempdir().unwrap();
        let follower = Follower {
            claude: dir.path().join("settings.json"),
            legacy_claude: dir.path().join(".claude.json"),
            codex: dir.path().join("config.toml"),
            profiles: dir.path().join("profiles"),
            last: None,
        };
        (dir, follower)
    }

    #[test]
    fn follows_saved_claude_changes_and_restores_terminal_on_leaving() {
        let (_dir, mut follower) = fixture();
        let now = Instant::now();
        std::fs::write(&follower.claude, r#"{"theme":"light-daltonized"}"#).unwrap();
        assert_eq!(follower.poll(Some("claude"), now), Some(Theme::Light));
        std::fs::write(&follower.claude, r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(
            follower.poll(Some("claude"), now + Duration::from_secs(1)),
            Some(Theme::Dark)
        );
        assert_eq!(follower.poll(None, now + Duration::from_secs(1)), None);
    }

    #[test]
    fn switching_agents_immediately_uses_their_own_theme() {
        let (_dir, mut follower) = fixture();
        let now = Instant::now();
        std::fs::write(&follower.claude, r#"{"theme":"light"}"#).unwrap();
        std::fs::write(&follower.codex, "[tui]\ntheme = 'catppuccin-mocha'\n").unwrap();
        assert_eq!(follower.poll(Some("claude"), now), Some(Theme::Light));
        assert_eq!(follower.poll(Some("codex"), now), Some(Theme::Dark));
        std::fs::write(&follower.codex, "[tui]\ntheme = 'catppuccin-latte'\n").unwrap();
        assert_eq!(
            follower.poll(Some("codex"), now + Duration::from_secs(1)),
            Some(Theme::Light)
        );
        assert_eq!(follower.poll(Some("shell"), now), None);
    }

    #[test]
    fn legacy_claude_is_fallback_but_auto_and_unknown_do_not_force_a_palette() {
        let (_dir, mut follower) = fixture();
        let now = Instant::now();
        std::fs::write(&follower.legacy_claude, r#"{"theme":"light"}"#).unwrap();
        assert_eq!(follower.poll(Some("claude"), now), Some(Theme::Light));
        std::fs::write(&follower.claude, r#"{"theme":"auto"}"#).unwrap();
        assert_eq!(
            follower.poll(Some("claude"), now + Duration::from_secs(1)),
            None
        );
        std::fs::write(&follower.codex, "[tui]\ntheme = 'my-custom-theme'\n").unwrap();
        assert_eq!(follower.poll(Some("codex"), now), None);
    }
}
