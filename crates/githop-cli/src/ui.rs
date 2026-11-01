//! Output styling, tables and prompts (Design §9.4).
//!
//! - Symbols `✓` (cyan), `✗` (red), `!` (yellow); hints are dim.
//! - Colour is off for `NO_COLOR`, `--no-color`, or when the stream is not a terminal.
//! - ASCII fallback `[ok]` / `[blocked]` / `[warn]` when `GITHOP_ASCII=1`. Rust writes
//!   to the Windows console with the wide-char API, so Unicode renders regardless of
//!   the console code page; the env var exists for fonts/log viewers that can't.
//! - Only the symbol is coloured; profile names and emails stay plain text.
//! - Prompts are only shown when stdin is a terminal.
//! - Every line is made terminal-safe before printing: control characters
//!   (escape sequences in a commit subject, a label, a path…) become `?`
//!   (security regression #2). JSON output is escaped by serde instead.
//! - Widths are display columns (wide CJK characters count 2, regression D15).

use std::io::{self, BufRead, IsTerminal, Write};

use githop_core::text::{char_width, display_width, terminal_safe};

use crate::cli::GlobalArgs;

/// Maximum line width for human output (Design §9.4).
pub const MAX_WIDTH: usize = 80;

/// Env var that forces the ASCII symbol fallback.
pub const ASCII_ENV: &str = "GITHOP_ASCII";

const CYAN: &str = "36";
const RED: &str = "31";
const YELLOW: &str = "33";
const DIM: &str = "2";

/// Kind of status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Blocked,
    Warn,
}

/// Terminal-related environment, captured once (pure inputs for the decisions below).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TermEnv {
    /// `NO_COLOR` is set to a non-empty value.
    pub no_color_env: bool,
    /// `CLICOLOR_FORCE` is set to something other than `0`.
    pub force_color: bool,
    /// `GITHOP_ASCII` is set to something other than `0`.
    pub ascii_env: bool,
    /// `TERM=dumb`.
    pub term_dumb: bool,
    /// The terminal understands ANSI escapes (Windows: VT processing enabled or a
    /// known ANSI terminal; elsewhere assumed).
    pub ansi_capable: bool,
}

fn env_flag(key: &str) -> bool {
    std::env::var_os(key).is_some_and(|v| !v.is_empty() && v != "0")
}

impl TermEnv {
    /// Read the process environment. On Windows this also switches on console
    /// virtual-terminal processing for stdout/stderr.
    pub fn from_process() -> Self {
        let term = std::env::var("TERM").unwrap_or_default();
        Self {
            no_color_env: std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()),
            force_color: env_flag("CLICOLOR_FORCE"),
            ascii_env: env_flag(ASCII_ENV),
            term_dumb: term == "dumb",
            ansi_capable: ansi_capable(&term),
        }
    }
}

/// Windows: try to enable VT processing; if the handle isn't a real console
/// (e.g. mintty pipes) fall back to known ANSI terminals. Elsewhere: true.
fn ansi_capable(term: &str) -> bool {
    match anstyle_query::windows::enable_ansi_colors() {
        None | Some(true) => true,
        Some(false) => {
            (!term.is_empty() && term != "dumb")
                || std::env::var_os("WT_SESSION").is_some()
                || std::env::var_os("TERM_PROGRAM").is_some()
                || std::env::var("ConEmuANSI").is_ok_and(|v| v == "ON")
                || std::env::var_os("ANSICON").is_some()
        }
    }
}

/// Should a stream get ANSI colour?
pub fn color_enabled(env: &TermEnv, no_color_flag: bool, is_tty: bool) -> bool {
    if no_color_flag || env.no_color_env {
        return false;
    }
    if env.force_color {
        return true;
    }
    is_tty && env.ansi_capable && !env.term_dumb
}

/// Should symbols use the ASCII fallback?
pub fn ascii_enabled(env: &TermEnv) -> bool {
    env.ascii_env
}

/// Symbol text for a status.
pub fn symbol(status: Status, ascii: bool) -> &'static str {
    match (status, ascii) {
        (Status::Ok, false) => "✓",
        (Status::Blocked, false) => "✗",
        (Status::Warn, false) => "!",
        (Status::Ok, true) => "[ok]",
        (Status::Blocked, true) => "[blocked]",
        (Status::Warn, true) => "[warn]",
    }
}

fn paint(text: &str, code: &str, color: bool) -> String {
    if color {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// Dim text (hints), when `color`.
pub fn paint_dim(text: &str, color: bool) -> String {
    paint(text, DIM, color)
}

/// `<symbol> <message>`, symbol coloured when `color`. The message is made
/// terminal-safe; in ASCII mode `—` and `…` become `-` and `...`.
pub fn status_line(status: Status, msg: &str, color: bool, ascii: bool) -> String {
    let code = match status {
        Status::Ok => CYAN,
        Status::Blocked => RED,
        Status::Warn => YELLOW,
    };
    format!(
        "{} {}",
        paint(symbol(status, ascii), code, color),
        safe(msg, ascii)
    )
}

/// Text ready to print: control characters neutralised, and in ASCII mode
/// the punctuation GitHop itself uses swapped for ASCII (regression D13).
pub fn safe(msg: &str, ascii: bool) -> String {
    let s = terminal_safe(msg);
    if ascii {
        s.replace('—', "-")
            .replace('…', "...")
            .replace('•', "*")
            .replace('→', "->")
    } else {
        s.into_owned()
    }
}

/// Number of display columns (wide characters count 2).
fn width(s: &str) -> usize {
    display_width(s)
}

/// Cut `s` to `max` columns, ending with `…` (or `...` in ASCII mode) when cut.
pub fn truncate(s: &str, max: usize, ascii: bool) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    let ellipsis = if ascii { "..." } else { "…" };
    let keep = max.saturating_sub(width(ellipsis));
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = char_width(c);
        if used + w > keep {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push_str(ellipsis);
    out
}

/// Render a left-aligned table with a header row, two spaces between columns,
/// no trailing spaces, total width ≤ `max_width` (widest columns shrink first).
pub fn render_table(
    headers: &[&str],
    rows: &[Vec<String>],
    max_width: usize,
    ascii: bool,
) -> String {
    let cols = headers.len();
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| r.iter().map(|c| one_line(c)).collect())
        .collect();
    let mut widths: Vec<usize> = headers.iter().map(|h| width(h)).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate().take(cols) {
            widths[i] = widths[i].max(width(cell));
        }
    }
    let gaps = 2 * cols.saturating_sub(1);
    let min_col = 6;
    while widths.iter().sum::<usize>() + gaps > max_width {
        let Some((idx, w)) = widths.iter().copied().enumerate().max_by_key(|&(_, w)| w) else {
            break;
        };
        if w <= min_col {
            break;
        }
        widths[idx] = w - 1;
    }

    let mut out = String::new();
    let mut push_row = |cells: Vec<String>| {
        let mut line = String::new();
        for (i, w) in widths.iter().enumerate() {
            let cell = truncate(cells.get(i).map(String::as_str).unwrap_or(""), *w, ascii);
            line.push_str(&cell);
            if i + 1 < cols {
                line.push_str(&" ".repeat(w - width(&cell) + 2));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    };
    push_row(headers.iter().map(|h| h.to_string()).collect());
    for row in rows {
        push_row(row);
    }
    out
}

/// A table cell on one line, terminal-safe.
fn one_line(s: &str) -> String {
    githop_core::text::one_line(s)
}

/// Output handle shared by every command.
#[derive(Debug, Clone)]
pub struct Ui {
    pub color_out: bool,
    pub color_err: bool,
    pub ascii: bool,
    pub quiet: bool,
    pub verbose: bool,
    pub json: bool,
    pub interactive: bool,
}

impl Ui {
    /// Decide styling from global flags and the process environment.
    pub fn new(args: &GlobalArgs) -> Self {
        let env = TermEnv::from_process();
        Self {
            color_out: color_enabled(&env, args.no_color, io::stdout().is_terminal()),
            color_err: color_enabled(&env, args.no_color, io::stderr().is_terminal()),
            ascii: ascii_enabled(&env),
            quiet: args.quiet,
            verbose: args.verbose,
            json: args.json,
            interactive: io::stdin().is_terminal(),
        }
    }

    /// Plain styling (tests).
    #[cfg(test)]
    pub fn plain() -> Self {
        Self {
            color_out: false,
            color_err: false,
            ascii: false,
            quiet: false,
            verbose: false,
            json: false,
            interactive: false,
        }
    }

    fn human(&self) -> bool {
        !self.json
    }

    /// `✓ msg` on stdout (suppressed by `-q` and `--json`).
    pub fn ok(&self, msg: &str) {
        if self.human() && !self.quiet {
            println!(
                "{}",
                status_line(Status::Ok, msg, self.color_out, self.ascii)
            );
        }
    }

    /// `✗ msg` on stdout (suppressed only by `--json`).
    pub fn blocked(&self, msg: &str) {
        if self.human() {
            println!(
                "{}",
                status_line(Status::Blocked, msg, self.color_out, self.ascii)
            );
        }
    }

    /// `! msg` on stdout (suppressed only by `--json`).
    pub fn warn(&self, msg: &str) {
        if self.human() {
            println!(
                "{}",
                status_line(Status::Warn, msg, self.color_out, self.ascii)
            );
        }
    }

    /// Dim hint line on stdout (suppressed by `-q` and `--json`).
    pub fn hint(&self, msg: &str) {
        if self.human() && !self.quiet {
            println!("{}", paint(&safe(msg, self.ascii), DIM, self.color_out));
        }
    }

    /// Dim hint line on stdout that `-q` keeps (the one next step of a
    /// warning or problem, Design §9.1; suppressed only by `--json`).
    pub fn fix_hint(&self, msg: &str) {
        if self.human() {
            println!("{}", paint(&safe(msg, self.ascii), DIM, self.color_out));
        }
    }

    /// Plain line on stdout (suppressed by `-q` and `--json`).
    pub fn line(&self, msg: &str) {
        if self.human() && !self.quiet {
            println!("{}", safe(msg, self.ascii));
        }
    }

    /// Extra detail, only with `-v`.
    pub fn detail(&self, msg: &str) {
        if self.human() && self.verbose {
            println!("{}", paint(&safe(msg, self.ascii), DIM, self.color_out));
        }
    }

    /// Table on stdout (suppressed by `-q` and `--json`).
    pub fn table(&self, headers: &[&str], rows: &[Vec<String>]) {
        if self.human() && !self.quiet {
            print!("{}", render_table(headers, rows, MAX_WIDTH, self.ascii));
        }
    }

    /// Problem line on stderr — always printed, even with `--json`/`-q`.
    pub fn error(&self, msg: &str) {
        eprintln!(
            "{}",
            status_line(Status::Blocked, msg, self.color_err, self.ascii)
        );
    }

    /// Plain line on stderr — always printed.
    pub fn error_line(&self, msg: &str) {
        eprintln!("{}", safe(msg, self.ascii));
    }

    /// Dim hint on stderr — always printed.
    pub fn error_hint(&self, msg: &str) {
        eprintln!("{}", paint(&safe(msg, self.ascii), DIM, self.color_err));
    }

    /// Print a JSON value on stdout (pretty, one document).
    pub fn json_out(&self, value: &serde_json::Value) -> anyhow::Result<()> {
        let text = serde_json::to_string_pretty(value)?;
        println!("{text}");
        Ok(())
    }

    /// Ask for a line of text. `default` is shown in brackets and used for an
    /// empty answer. Only call when `self.interactive`.
    pub fn prompt(&self, label: &str, default: Option<&str>) -> anyhow::Result<String> {
        let mut out = io::stdout().lock();
        match default {
            Some(d) if !d.is_empty() => write!(out, "{label} [{d}]: ")?,
            _ => write!(out, "{label}: ")?,
        }
        out.flush()?;
        drop(out);
        let mut answer = String::new();
        let read = io::stdin().lock().read_line(&mut answer)?;
        if read == 0 {
            return Err(crate::exit::usage("No answer was given (input closed)."));
        }
        let answer = answer.trim().to_string();
        Ok(if answer.is_empty() {
            default.unwrap_or("").to_string()
        } else {
            answer
        })
    }

    /// Yes/no question. Only call when `self.interactive`.
    pub fn confirm(&self, question: &str, default_yes: bool) -> anyhow::Result<bool> {
        let suffix = if default_yes { "[Y/n]" } else { "[y/N]" };
        let answer = self.prompt(&format!("{question} {suffix}"), None)?;
        Ok(match answer.to_ascii_lowercase().as_str() {
            "" => default_yes,
            "y" | "yes" => true,
            _ => false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tty_env() -> TermEnv {
        TermEnv {
            ansi_capable: true,
            ..TermEnv::default()
        }
    }

    #[test]
    fn colour_on_for_capable_tty() {
        assert!(color_enabled(&tty_env(), false, true));
    }

    #[test]
    fn colour_off_when_not_tty() {
        assert!(!color_enabled(&tty_env(), false, false));
    }

    #[test]
    fn colour_off_for_no_color_env_and_flag() {
        let env = TermEnv {
            no_color_env: true,
            ..tty_env()
        };
        assert!(!color_enabled(&env, false, true));
        assert!(!color_enabled(&tty_env(), true, true));
    }

    #[test]
    fn no_color_beats_force() {
        let env = TermEnv {
            no_color_env: true,
            force_color: true,
            ..tty_env()
        };
        assert!(!color_enabled(&env, false, true));
        let forced = TermEnv {
            force_color: true,
            ..TermEnv::default()
        };
        assert!(color_enabled(&forced, false, false));
    }

    #[test]
    fn colour_off_for_dumb_or_non_ansi_terminal() {
        let dumb = TermEnv {
            term_dumb: true,
            ..tty_env()
        };
        assert!(!color_enabled(&dumb, false, true));
        assert!(!color_enabled(&TermEnv::default(), false, true));
    }

    #[test]
    fn ascii_fallback_symbols() {
        let env = TermEnv {
            ascii_env: true,
            ..TermEnv::default()
        };
        assert!(ascii_enabled(&env));
        assert!(!ascii_enabled(&TermEnv::default()));
        assert_eq!(symbol(Status::Ok, true), "[ok]");
        assert_eq!(symbol(Status::Blocked, true), "[blocked]");
        assert_eq!(symbol(Status::Warn, true), "[warn]");
        assert_eq!(symbol(Status::Ok, false), "✓");
        assert_eq!(symbol(Status::Blocked, false), "✗");
        assert_eq!(symbol(Status::Warn, false), "!");
    }

    #[test]
    fn status_line_colours_only_the_symbol() {
        assert_eq!(
            status_line(Status::Ok, "Pinned to Office", false, false),
            "✓ Pinned to Office"
        );
        assert_eq!(
            status_line(Status::Blocked, "x@y", true, false),
            "\x1b[31m✗\x1b[0m x@y"
        );
        assert_eq!(
            status_line(Status::Warn, "w", true, true),
            "\x1b[33m[warn]\x1b[0m w"
        );
        assert_eq!(
            status_line(Status::Ok, "o", true, false),
            "\x1b[36m✓\x1b[0m o"
        );
    }

    #[test]
    fn truncate_cases() {
        assert_eq!(truncate("short", 10, false), "short");
        assert_eq!(truncate("abcdefghij", 6, false), "abcde…");
        assert_eq!(truncate("abcdefghij", 6, true), "abc...");
    }

    #[test]
    fn table_aligns_columns() {
        let rows = vec![
            vec!["office".to_string(), "Office".to_string()],
            vec!["personal".to_string(), "Personal".to_string()],
        ];
        let t = render_table(&["Id", "Label"], &rows, 80, false);
        assert_eq!(t, "Id        Label\noffice    Office\npersonal  Personal\n");
    }

    #[test]
    fn table_has_no_trailing_spaces_and_handles_short_rows() {
        let rows = vec![vec!["a".to_string()], vec!["b".to_string(), String::new()]];
        let t = render_table(&["One", "Two"], &rows, 80, false);
        assert!(t.lines().all(|l| !l.ends_with(' ')));
        assert_eq!(t.lines().count(), 3);
    }

    /// Regression D15: wide characters count two columns.
    #[test]
    fn table_aligns_wide_characters() {
        let rows = vec![
            vec![
                "work".to_string(),
                "Ünïcødé Wörk 日本".to_string(),
                "x".into(),
            ],
            vec!["home".to_string(), "Home".to_string(), "y".into()],
        ];
        let t = render_table(&["Id", "Label", "End"], &rows, 80, false);
        let cols: Vec<usize> = t
            .lines()
            .map(|l| {
                let i = l.rfind(' ').unwrap();
                display_width(&l[..=i])
            })
            .collect();
        assert!(cols.iter().all(|c| *c == cols[0]), "{t}\n{cols:?}");
        assert_eq!(truncate("日本語テキスト", 7, false), "日本語…");
    }

    /// Security regression #2: escape sequences never reach the terminal.
    #[test]
    fn control_characters_are_neutralised() {
        let evil = "fix\x1b]0;OWNED\x07\x1b[2J\x1b[31mFAKE";
        let line = status_line(Status::Blocked, evil, false, false);
        assert!(!line.contains('\x1b') && !line.contains('\x07'), "{line:?}");
        let t = render_table(&["Subject"], &[vec![evil.to_string()]], 80, false);
        assert!(!t.contains('\x1b'), "{t:?}");
        assert_eq!(safe("a — b…", true), "a - b...");
    }

    #[test]
    fn table_fits_max_width() {
        let long = "x".repeat(120);
        let rows = vec![vec!["id".to_string(), long.clone(), "end".to_string()]];
        let t = render_table(&["Id", "Path", "Status"], &rows, 80, false);
        for line in t.lines() {
            assert!(width(line) <= 80, "line too wide: {line}");
        }
        assert!(t.contains('…'));
        assert!(t.contains("end"));
    }
}
