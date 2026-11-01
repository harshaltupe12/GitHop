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
use crate::ui::{paint_dim, status_line, Status, Ui};

/// Upper bound on pre-push stdin (one line per ref; generous).
const MAX_STDIN: u64 = 4 * 1024 * 1024;

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
    let mut buf = Vec::new();
    stdin.lock().take(MAX_STDIN).read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
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
            Level::Info => m.text.clone(),
        };
        out.push(line);
        if let Some(hint) = &m.hint {
            out.push(paint_dim(hint, color));
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
    }

    #[test]
    fn exit_codes_map() {
        assert_eq!(exit_for(0), Exit::Ok);
        assert_eq!(exit_for(1), Exit::Blocked);
        assert_eq!(exit_for(3), Exit::Internal);
        assert_eq!(exit_for(-1), Exit::Internal);
    }
}
