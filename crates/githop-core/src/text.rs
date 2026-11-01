//! Small text helpers shared by the CLI and core message builders: plurals,
//! terminal-safe text and display width (Design §9.1, §9.4).

use std::borrow::Cow;

/// `1 repo`, `2 repos`, `0 repos`.
pub fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// A character a terminal would act on rather than print: C0 controls, DEL
/// and C1 controls (`ESC`, `CSI`, `OSC`, `BEL`, …), plus the Unicode bidi
/// overrides that can make a line read differently than it is.
fn is_unsafe(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Text from a repo, a profile file or the user (commit subjects, emails, ref
/// names, labels, paths) made safe to print: every control character becomes
/// `?`, except `\n` and `\t`, so an escape sequence can't clear, recolour or
/// retitle the terminal (security regression #2). Borrowed when already safe.
pub fn terminal_safe(s: &str) -> Cow<'_, str> {
    if !s.chars().any(|c| is_unsafe(c) && c != '\n' && c != '\t') {
        return Cow::Borrowed(s);
    }
    Cow::Owned(
        s.chars()
            .map(|c| {
                if is_unsafe(c) && c != '\n' && c != '\t' {
                    '?'
                } else {
                    c
                }
            })
            .collect(),
    )
}

/// Like [`terminal_safe`], for a value that must stay on one line: `\n` and
/// `\r` become `?` too, and `\t` a space.
pub fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\t' => ' ',
            c if is_unsafe(c) => '?',
            c => c,
        })
        .collect()
}

/// Display columns of `c` in a terminal: 0 for combining marks and
/// zero-width characters, 2 for East Asian Wide and Fullwidth characters
/// (CJK, Hangul, fullwidth forms, most emoji), else 1. A compact version of
/// Unicode's East Asian Width table (UAX #11), enough for table alignment.
pub fn char_width(c: char) -> usize {
    let cp = c as u32;
    if cp == 0 || is_zero_width(cp) {
        return 0;
    }
    if is_wide(cp) {
        2
    } else {
        1
    }
}

fn is_zero_width(cp: u32) -> bool {
    matches!(cp,
        0x0300..=0x036F   // combining diacritical marks
        | 0x0483..=0x0489
        | 0x0591..=0x05BD
        | 0x0610..=0x061A
        | 0x064B..=0x065F
        | 0x0E31 | 0x0E34..=0x0E3A | 0x0E47..=0x0E4E
        | 0x1AB0..=0x1AFF
        | 0x1DC0..=0x1DFF
        | 0x200B..=0x200F // zero-width space, joiners, marks
        | 0x2028..=0x202E
        | 0x2060..=0x2064
        | 0x20D0..=0x20FF
        | 0xFE00..=0xFE0F // variation selectors
        | 0xFE20..=0xFE2F
        | 0xFEFF
        | 0xE0100..=0xE01EF)
}

fn is_wide(cp: u32) -> bool {
    matches!(cp,
        0x1100..=0x115F   // Hangul Jamo
        | 0x231A..=0x231B
        | 0x2329..=0x232A
        | 0x23E9..=0x23EC | 0x23F0 | 0x23F3
        | 0x25FD..=0x25FE
        | 0x2614..=0x2615
        | 0x2648..=0x2653
        | 0x267F | 0x2693 | 0x26A1 | 0x26AA..=0x26AB | 0x26BD..=0x26BE
        | 0x26C4..=0x26C5 | 0x26CE | 0x26D4 | 0x26EA | 0x26F2..=0x26F3
        | 0x26F5 | 0x26FA | 0x26FD | 0x2705 | 0x270A..=0x270B | 0x2728
        | 0x274C | 0x274E | 0x2753..=0x2755 | 0x2757 | 0x2795..=0x2797
        | 0x27B0 | 0x27BF | 0x2B1B..=0x2B1C | 0x2B50 | 0x2B55
        | 0x2E80..=0x303E // CJK radicals, punctuation
        | 0x3041..=0x33FF // kana, CJK symbols
        | 0x3400..=0x4DBF // CJK ext A
        | 0x4E00..=0x9FFF // CJK unified
        | 0xA000..=0xA4CF // Yi
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7A3 // Hangul syllables
        | 0xF900..=0xFAFF // CJK compatibility
        | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60 // fullwidth forms
        | 0xFFE0..=0xFFE6
        | 0x1F004 | 0x1F0CF | 0x1F18E | 0x1F191..=0x1F19A
        | 0x1F200..=0x1F251
        | 0x1F300..=0x1F64F // emoji
        | 0x1F680..=0x1F6FF
        | 0x1F7E0..=0x1F7EB
        | 0x1F90C..=0x1F9FF
        | 0x1FA70..=0x1FAFF
        | 0x20000..=0x2FFFD // CJK ext B+
        | 0x30000..=0x3FFFD)
}

/// Display columns of `s` (see [`char_width`]).
pub fn display_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// Wrap `text` at spaces so no line is wider than `max` columns (a word
/// longer than `max`, e.g. a path, gets a line of its own). `indent` is put
/// before every line after the first.
pub fn wrap(text: &str, max: usize, indent: &str) -> String {
    let mut out = String::new();
    let mut line = String::new();
    let mut first = true;
    for word in text.split(' ').filter(|w| !w.is_empty()) {
        let prefix = if first { "" } else { indent };
        let candidate = if line.is_empty() {
            display_width(prefix) + display_width(word)
        } else {
            display_width(prefix) + display_width(&line) + 1 + display_width(word)
        };
        if !line.is_empty() && candidate > max {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(prefix);
            out.push_str(&line);
            line.clear();
            first = false;
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(if first { "" } else { indent });
        out.push_str(&line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plurals() {
        assert_eq!(plural(1, "repo", "repos"), "1 repo");
        assert_eq!(plural(0, "repo", "repos"), "0 repos");
        assert_eq!(plural(4, "problem", "problems"), "4 problems");
    }

    #[test]
    fn escape_sequences_are_neutralised() {
        let evil = "fix typo\x1b]0;OWNED\x07\x1b[2J\x1b[31mFAKE\x1b[0m\u{9b}x";
        let safe = terminal_safe(evil);
        assert!(!safe.chars().any(|c| c.is_control()), "{safe:?}");
        assert!(safe.contains("fix typo"));
        assert_eq!(terminal_safe("plain\tline\nnext"), "plain\tline\nnext");
        assert!(matches!(terminal_safe("ok"), Cow::Borrowed(_)));
        assert_eq!(one_line("a\nb\tc\x1b"), "a?b c?");
        assert_eq!(terminal_safe("a\u{202E}b"), "a?b");
    }

    #[test]
    fn widths() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("日本"), 4);
        assert_eq!(display_width("Ünïcødé"), 7);
        assert_eq!(display_width("e\u{301}"), 1);
        assert_eq!(display_width("ｶ"), 1, "halfwidth katakana is narrow");
        assert_eq!(display_width("Ａ"), 2, "fullwidth A is wide");
    }

    #[test]
    fn wrapping() {
        let w = wrap("one two three four five", 9, "  ");
        assert_eq!(w, "one two\n  three\n  four\n  five");
        assert!(wrap(&"x ".repeat(100), 80, "")
            .lines()
            .all(|l| display_width(l) <= 80));
        assert_eq!(wrap("short", 80, "  "), "short");
    }
}
