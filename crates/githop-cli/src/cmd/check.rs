//! `githop check --stage <clone|commit|push> [-- <hook args>...]` — the hook
//! entry point (Phase 3 §4.2).
//!
//! - Never prompts. Reads stdin only for `push` (the ref lines).
//! - Messages go to **stderr** (git shows hook stderr; stdout may be captured).
//!   Colour only when stderr is a terminal (hooks often run inside IDEs).
//! - Exit code comes from the core: `0` ok · `1` blocked · `3` GitHop's own
//!   error (the hook fails open). `--json` also prints the result on stdout.
//! - Keep it lean: no store or git lookup here — the core does exactly the
//!   work the stage needs (Plan §11 spawn budget).

use std::io::{IsTerminal, Read};

use anyhow::Result;
use githop_core::check::{self, CheckResult, Level, Message, Stage as CoreStage};
use githop_core::EnforceMode;

use super::Ctx;
use crate::cli::{CheckArgs, Stage};
use crate::exit::Exit;
use crate::ui::{paint_dim, safe, status_line, Status, Ui};

/// Upper bound on pre-push stdin: one ~100-byte line per ref, so ~600k refs.
/// Past it the check fails (fail open, or blocked under block-strict) instead
/// of silently checking only the first refs (review 2026-11-01 #10).
const MAX_STDIN: u64 = 64 * 1024 * 1024;

pub fn core_stage(stage: Stage) -> CoreStage {
    match stage {
        Stage::Clone => CoreStage::Clone,
        Stage::Commit => CoreStage::Commit,
        Stage::Push => CoreStage::Push,
    }
}

pub fn run(ctx: &Ctx, args: CheckArgs) -> Result<Exit> {
    let stage = core_stage(args.stage);
    let result = match read_stdin(stage) {
        Ok(stdin) => check::run_check_from_env(stage, &args.hook_args, &stdin),
        Err(e) => CheckResult::internal_error(
            stage,
            check_enforce_mode(),
            format!("couldn't read the hook input: {e}"),
        ),
    };
    render(&ctx.ui, &result);
    if ctx.ui.json {
        // Best effort: the exit code is what hooks rely on.
        if let Ok(v) = serde_json::to_value(&result) {
            let _ = ctx.ui.json_out(&v);
        }
    }
    Ok(exit_for(result.exit_code()))
}

/// Enforce mode for a failure before the core could read it.
fn check_enforce_mode() -> EnforceMode {
    githop_core::Paths::resolve()
        .map(|p| check::load_settings_lenient(&p).enforce)
        .unwrap_or_default()
}

/// pre-push gets `<local ref> <local sha> <remote ref> <remote sha>` lines on
/// stdin. Other stages never read stdin (it may be the user's terminal).
fn read_stdin(stage: CoreStage) -> std::io::Result<String> {
    if stage != CoreStage::Push {
        return Ok(String::new());
    }
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Ok(String::new());
    }
    let buf = read_bounded(&mut stdin.lock(), MAX_STDIN)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Read all of `r`, up to `max` bytes. More than that is an error (after
/// draining the rest, so git never sees a broken pipe): a ref list is never
/// cut short silently.
fn read_bounded(r: &mut impl Read, max: u64) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    r.take(max.saturating_add(1)).read_to_end(&mut buf)?;
    if buf.len() as u64 > max {
        let _ = std::io::copy(r, &mut std::io::sink());
        return Err(std::io::Error::other(format!(
            "the push lists too many refs to check (more than {} MiB)",
            max / (1024 * 1024)
        )));
    }
    Ok(buf)
}

fn exit_for(code: i32) -> Exit {
    match code {
        0 => Exit::Ok,
        1 => Exit::Blocked,
        2 => Exit::Usage,
        _ => Exit::Internal,
    }
}

/// Lines for the messages, as printed on stderr (Design §9.3–9.4).
pub fn render_lines(messages: &[Message], color: bool, ascii: bool) -> Vec<String> {
    let mut out = Vec::new();
    for m in messages {
        let line = match m.level {
            Level::Ok => status_line(Status::Ok, &m.text, color, ascii),
            Level::Blocked => status_line(Status::Blocked, &m.text, color, ascii),
            Level::Warning => status_line(Status::Warn, &m.text, color, ascii),
            Level::Info => safe(&m.text, ascii),
        };
        out.push(line);
        if let Some(hint) = &m.hint {
            out.push(paint_dim(&safe(hint, ascii), color));
        }
    }
    out
}

fn render(ui: &Ui, result: &CheckResult) {
    if result.messages.is_empty() {
        return;
    }
    let text = render_lines(&result.messages, ui.color_err, ui.ascii).join("\n");
    eprintln!("{text}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_commit_renders_like_design() {
        let msgs = vec![
            Message::blocked("GitHop: commit blocked"),
            Message::info("This repo belongs to Office."),
            Message::info("Commit would use harshal@personal.").hint("Run: githop pin office"),
        ];
        assert_eq!(
            render_lines(&msgs, false, false),
            vec![
                "✗ GitHop: commit blocked",
                "This repo belongs to Office.",
                "Commit would use harshal@personal.",
                "Run: githop pin office",
            ]
        );
        let coloured = render_lines(&msgs, true, false);
        assert_eq!(coloured[0], "\x1b[31m✗\x1b[0m GitHop: commit blocked");
        assert_eq!(coloured[3], "\x1b[2mRun: githop pin office\x1b[0m");
        assert_eq!(
            render_lines(&[Message::ok("GitHop: pinned to Personal")], false, true),
            vec!["[ok] GitHop: pinned to Personal"]
        );
        assert_eq!(
            render_lines(&[Message::warning("w")], false, false),
            vec!["! w"]
        );
        // Regression D13: ASCII mode has no em dash or ellipsis either.
        assert_eq!(
            render_lines(
                &[Message::blocked("GitHop: push blocked — 1 commit…").hint("x — y")],
                false,
                true
            ),
            vec!["[blocked] GitHop: push blocked - 1 commit...", "x - y"]
        );
        // Security regression #2: a commit subject can't drive the terminal.
        let evil = Message::info("  1111111  fix\x1b]0;T\x07\x1b[2J  a@b");
        let lines = render_lines(&[evil], true, false);
        assert!(!lines[0].contains('\x1b') && !lines[0].contains('\x07'));
    }

    /// Review 2026-11-01 #10: over the cap is an error, never a silent cut.
    #[test]
    fn stdin_over_the_cap_is_an_error_not_a_truncation() {
        let lines = "refs/heads/a 1111 refs/heads/a 0000\n".repeat(100);
        let mut ok = std::io::Cursor::new(lines.clone().into_bytes());
        assert_eq!(
            read_bounded(&mut ok, lines.len() as u64).unwrap(),
            lines.as_bytes()
        );
        let mut over = std::io::Cursor::new(lines.clone().into_bytes());
        let err = read_bounded(&mut over, lines.len() as u64 - 1).unwrap_err();
        assert!(err.to_string().contains("too many refs"), "{err}");
        assert_eq!(
            over.position(),
            lines.len() as u64,
            "the rest is drained so git sees no broken pipe"
        );
        // The error becomes GitHop's own failure: fail open, or blocked under block-strict.
        let r = CheckResult::internal_error(CoreStage::Push, EnforceMode::Block, err.to_string());
        assert_eq!(r.exit_code(), 3);
        let r =
            CheckResult::internal_error(CoreStage::Push, EnforceMode::BlockStrict, err.to_string());
        assert_eq!(r.exit_code(), 1);
    }

    #[test]
    fn exit_codes_map() {
        assert_eq!(exit_for(0), Exit::Ok);
        assert_eq!(exit_for(1), Exit::Blocked);
        assert_eq!(exit_for(3), Exit::Internal);
        assert_eq!(exit_for(-1), Exit::Internal);
    }
}
