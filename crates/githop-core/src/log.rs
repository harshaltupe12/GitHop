//! Opt-in check log (Phase 3 §4.6, Plan §9.2).
//!
//! Only when `settings.logging = true`: one line per check appended to
//! `logs/githop-YYYY-MM-DD.log` (UTC date):
//!
//! ```text
//! 2026-10-31T14:22:33Z stage=commit profile=office result=blocked
//! ```
//!
//! Never written: repo paths, URLs, emails, commit messages. The profile id is
//! a slug (`[a-z0-9-]`); anything else is logged as `?`. Files older than
//! `log_retention_days` are deleted when the day's file is first created
//! (so the cleanup runs at most once a day).

use std::io::Write;
use std::path::Path;

use time::macros::format_description;
use time::{Date, Month, OffsetDateTime};

use crate::check::Stage;
use crate::error::{CoreError, Result};
use crate::paths::Paths;
use crate::settings::Settings;

const PREFIX: &str = "githop-";
const SUFFIX: &str = ".log";

/// Keep only slug characters; anything else → `?` (the log must never carry data).
fn safe_token(s: &str) -> String {
    if !s.is_empty()
        && s.len() <= 32
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        s.to_string()
    } else {
        "?".to_string()
    }
}

/// One log line (no trailing newline). Pure.
pub fn format_line(
    at: OffsetDateTime,
    stage: Stage,
    profile_id: Option<&str>,
    result: &str,
) -> String {
    let ts = at
        .to_offset(time::UtcOffset::UTC)
        .format(format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second]Z"
        ))
        .unwrap_or_else(|_| "?".to_string());
    format!(
        "{ts} stage={} profile={} result={}",
        stage.as_str(),
        profile_id
            .map(safe_token)
            .unwrap_or_else(|| "-".to_string()),
        safe_token(result)
    )
}

/// `githop-YYYY-MM-DD.log` for `date`.
pub fn file_name(date: Date) -> String {
    format!(
        "{PREFIX}{:04}-{:02}-{:02}{SUFFIX}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

/// Parse the date out of a log file name. `None` for anything else.
pub fn parse_file_date(name: &str) -> Option<Date> {
    let d = name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
    let mut it = d.split('-');
    let (y, m, day) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some() || y.len() != 4 || m.len() != 2 || day.len() != 2 {
        return None;
    }
    let month = Month::try_from(m.parse::<u8>().ok()?).ok()?;
    Date::from_calendar_date(y.parse().ok()?, month, day.parse().ok()?).ok()
}

/// Log files in `dir` older than `retention_days` relative to `today`. Pure apart from listing the dir.
pub fn expired_files(dir: &Path, today: Date, retention_days: u32) -> Vec<std::path::PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let date = parse_file_date(&name)?;
            ((today - date).whole_days() > i64::from(retention_days)).then(|| e.path())
        })
        .collect()
}

/// Append one line for a check if logging is on. Cleanup of old files runs
/// when today's file is created. Errors are returned; callers in hooks ignore them.
pub fn append(
    paths: &Paths,
    settings: &Settings,
    stage: Stage,
    profile_id: Option<&str>,
    result: &str,
) -> Result<()> {
    append_at(
        paths,
        settings,
        OffsetDateTime::now_utc(),
        stage,
        profile_id,
        result,
    )
}

/// [`append`] with an explicit time (tests).
pub fn append_at(
    paths: &Paths,
    settings: &Settings,
    now: OffsetDateTime,
    stage: Stage,
    profile_id: Option<&str>,
    result: &str,
) -> Result<()> {
    if !settings.logging {
        return Ok(());
    }
    let dir = paths.logs_dir();
    std::fs::create_dir_all(&dir).map_err(|e| CoreError::io(&dir, e))?;
    let today = now.to_offset(time::UtcOffset::UTC).date();
    let file = dir.join(file_name(today));
    let first_today = !file.exists();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .map_err(|e| CoreError::io(&file, e))?;
    let line = format_line(now, stage, profile_id, result);
    f.write_all(format!("{line}\n").as_bytes())
        .map_err(|e| CoreError::io(&file, e))?;
    if first_today {
        for old in expired_files(&dir, today, settings.log_retention_days) {
            let _ = std::fs::remove_file(old);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn line_format() {
        let l = format_line(
            datetime!(2026-10-31 14:22:33 UTC),
            Stage::Commit,
            Some("office"),
            "blocked",
        );
        assert_eq!(
            l,
            "2026-10-31T14:22:33Z stage=commit profile=office result=blocked"
        );
        let l = format_line(
            datetime!(2026-10-31 14:22:33 +02:00),
            Stage::Push,
            None,
            "pass",
        );
        assert_eq!(l, "2026-10-31T12:22:33Z stage=push profile=- result=pass");
    }

    #[test]
    fn line_never_carries_data() {
        let l = format_line(
            datetime!(2026-10-31 0:00 UTC),
            Stage::Clone,
            Some("me@personal.example"),
            "C:/Users/me/repo https://github.com/x/y",
        );
        assert!(!l.contains('@'));
        assert!(!l.contains('/'));
        assert!(!l.contains("github"));
        assert!(l.ends_with("profile=? result=?"));
    }

    #[test]
    fn file_names_roundtrip() {
        let d = Date::from_calendar_date(2026, Month::March, 5).unwrap();
        assert_eq!(file_name(d), "githop-2026-03-05.log");
        assert_eq!(parse_file_date("githop-2026-03-05.log"), Some(d));
        for bad in [
            "githop-2026-3-5.log",
            "other.log",
            "githop-2026-13-01.log",
            "githop-2026-03-05.txt",
        ] {
            assert_eq!(parse_file_date(bad), None, "{bad}");
        }
    }

    #[test]
    fn append_and_retention() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::at(tmp.path());
        let mut settings = Settings::default();
        // Off by default: nothing written.
        append_at(
            &paths,
            &settings,
            datetime!(2026-10-31 9:00 UTC),
            Stage::Commit,
            None,
            "pass",
        )
        .unwrap();
        assert!(!paths.logs_dir().exists());

        settings.logging = true;
        settings.log_retention_days = 7;
        std::fs::create_dir_all(paths.logs_dir()).unwrap();
        let old = paths.logs_dir().join("githop-2026-10-20.log");
        let recent = paths.logs_dir().join("githop-2026-10-25.log");
        let foreign = paths.logs_dir().join("notes.txt");
        for p in [&old, &recent, &foreign] {
            std::fs::write(p, "x\n").unwrap();
        }
        append_at(
            &paths,
            &settings,
            datetime!(2026-10-31 9:00 UTC),
            Stage::Commit,
            Some("office"),
            "pass",
        )
        .unwrap();
        append_at(
            &paths,
            &settings,
            datetime!(2026-10-31 9:01 UTC),
            Stage::Push,
            Some("office"),
            "blocked",
        )
        .unwrap();
        assert!(!old.exists());
        assert!(recent.exists());
        assert!(foreign.exists());
        let today =
            std::fs::read_to_string(paths.logs_dir().join("githop-2026-10-31.log")).unwrap();
        assert_eq!(
            today,
            "2026-10-31T09:00:00Z stage=commit profile=office result=pass\n\
             2026-10-31T09:01:00Z stage=push profile=office result=blocked\n"
        );
    }
}
