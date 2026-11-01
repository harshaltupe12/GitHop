//! `githop installer path-add <dir>` / `path-remove <dir>` — hidden helpers
//! for the Windows installer (Phase 6 §5).
//!
//! The installer used to edit the user PATH with NSIS string functions, which
//! are limited to 1024 characters and silently truncated or replaced long
//! PATHs. The edit now happens here, with no length limit:
//!
//! * Only `HKCU\Environment` `Path` is touched (never the machine PATH).
//! * The value keeps its type (`REG_EXPAND_SZ` or `REG_SZ`), and every other
//!   entry is kept byte for byte, in order — including `%VAR%` entries (never
//!   expanded), empty entries and a trailing `;`.
//! * Our folder is matched case-insensitively, ignoring a trailing `\` and
//!   quotes, so it is added once and every copy is removed.
//! * When the value can't be read with certainty (unbalanced quotes, an
//!   unexpected type, an unreadable export), nothing is written and the
//!   command exits `2` so the installer can tell the user to edit PATH by hand.
//!
//! Registry access goes through `reg.exe` with argument arrays (CLAUDE.md
//! rule 9): `reg export` gives the exact UTF-16 data and type, and
//! `reg import` writes it back without command-line quoting or length limits.
//! Every write is read back and compared; on a mismatch the old value is
//! written back.
//!
//! Tests: `GITHOP_PATH_KEY` points the commands at a throwaway key, accepted
//! only under `HKCU\Software\GitHopTest`. With `GITHOP_NO_REGISTRY=1` and no
//! test key, the commands change nothing.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, Result};
use clap::{Args, Subcommand};
use githop_core::wipe::{NO_REGISTRY_ENV, TEST_KEY_PREFIX};

use super::Ctx;
use crate::exit::{usage_hint, Exit};

/// Env var that points the PATH commands at a test key (must start with [`TEST_KEY_PREFIX`]).
pub const PATH_KEY_ENV: &str = "GITHOP_PATH_KEY";

/// The per-user environment key.
pub const USER_ENV_KEY: &str = r"HKCU\Environment";

/// Value name Windows reads (names are case-insensitive).
const PATH_VALUE: &str = "Path";

/// Longest environment value Windows can use (UTF-16 units, without the NUL).
pub const MAX_PATH_VALUE_LEN: usize = 32_766;

#[derive(Debug, Subcommand)]
pub enum InstallerCommand {
    /// Add a folder to the user PATH (once)
    PathAdd(DirArg),
    /// Remove a folder from the user PATH
    PathRemove(DirArg),
}

#[derive(Debug, Clone, Args)]
pub struct DirArg {
    /// Folder that holds githop.exe
    pub dir: String,
}

pub fn run(ctx: &Ctx, command: InstallerCommand) -> Result<Exit> {
    let (adding, dir) = match &command {
        InstallerCommand::PathAdd(a) => (true, a.dir.as_str()),
        InstallerCommand::PathRemove(a) => (false, a.dir.as_str()),
    };
    let ui = &ctx.ui;
    let dir = clean_dir(dir).map_err(|e| refusal(adding, dir, &e))?;

    let key = match target_key()? {
        Some(k) => k,
        None => {
            ui.line(&format!(
                "Skipped the PATH change ({NO_REGISTRY_ENV}=1). Nothing was changed."
            ));
            return Ok(Exit::Ok);
        }
    };
    if !cfg!(windows) {
        ui.line("There's no Windows PATH to change on this system.");
        return Ok(Exit::Ok);
    }

    let current = read_value(&key).map_err(|e| refusal(adding, &dir, &e))?;
    let edit = if adding {
        add_entry(current.as_ref().map(|v| v.data.as_str()), &dir)
    } else {
        remove_entry(current.as_ref().map(|v| v.data.as_str()), &dir)
    }
    .map_err(|e| refusal(adding, &dir, &e))?;

    match edit {
        Edit::Unchanged => {
            if adding {
                ui.ok(&format!("{dir} is already on your PATH."));
            } else {
                ui.ok(&format!("{dir} wasn't on your PATH."));
            }
            return Ok(Exit::Ok);
        }
        Edit::Set(data) => {
            let new = RegValue {
                name: current
                    .as_ref()
                    .map(|v| v.name.clone())
                    .unwrap_or_else(|| PATH_VALUE.to_string()),
                kind: current.as_ref().map(|v| v.kind).unwrap_or(RegKind::ExpandSz),
                data,
            };
            write_verified(&key, &new, current.as_ref())?;
        }
        Edit::Delete => {
            let name = current
                .as_ref()
                .map(|v| v.name.clone())
                .unwrap_or_else(|| PATH_VALUE.to_string());
            delete_verified(&key, &name, current.as_ref())?;
        }
    }
    if adding {
        ui.ok(&format!(
            "Added {dir} to your PATH. Open a new terminal to use githop."
        ));
    } else {
        ui.ok(&format!("Removed {dir} from your PATH."));
    }
    Ok(Exit::Ok)
}

/// Exit-2 error: PATH wasn't changed, change it by hand.
fn refusal(adding: bool, dir: &str, why: &PathError) -> anyhow::Error {
    let hint = if adding {
        format!("To use githop in terminals, add {dir} to your PATH by hand.")
    } else {
        format!("If {dir} is on your PATH, remove it by hand.")
    };
    usage_hint(format!("Your PATH wasn't changed: {why}"), hint)
}

/// The key to edit: the user environment, or a validated test key.
/// `None` when registry access is switched off for tests.
fn target_key() -> Result<Option<String>> {
    if let Some(raw) = std::env::var_os(PATH_KEY_ENV) {
        let k = raw.to_string_lossy().trim().to_string();
        if !is_test_key(&k) {
            return Err(usage_hint(
                format!("{PATH_KEY_ENV} must name a key under {TEST_KEY_PREFIX}."),
                format!("Unset {PATH_KEY_ENV} to change your real PATH."),
            ));
        }
        return Ok(Some(k));
    }
    if std::env::var_os(NO_REGISTRY_ENV).is_some_and(|v| v.to_string_lossy().trim() == "1") {
        return Ok(None);
    }
    Ok(Some(USER_ENV_KEY.to_string()))
}

/// `HKCU\Software\GitHopTest…` (longer than the prefix, no `..`).
pub fn is_test_key(k: &str) -> bool {
    k.len() > TEST_KEY_PREFIX.len()
        && k.get(..TEST_KEY_PREFIX.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(TEST_KEY_PREFIX))
        && !k.contains("..")
        && !k.contains(['"', '\0', '/'])
}

// ---------------------------------------------------------------------------
// PATH transform (pure)
// ---------------------------------------------------------------------------

/// Why the PATH was left alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// The folder argument can't be put on PATH as is.
    BadDir(&'static str),
    /// The PATH has an odd number of `"`, so its entries can't be split safely.
    UnbalancedQuotes,
    /// The result would be longer than Windows allows.
    TooLong(usize),
    /// `Path` isn't a string value (e.g. `REG_MULTI_SZ`).
    UnsupportedType(String),
    /// The value contains a NUL character.
    EmbeddedNul,
    /// `reg.exe` failed or its output couldn't be read with certainty.
    Unreadable(String),
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathError::BadDir(why) => write!(f, "the folder {why}."),
            PathError::UnbalancedQuotes => {
                f.write_str("it has an unmatched quote, so its entries can't be told apart.")
            }
            PathError::TooLong(n) => write!(
                f,
                "it would be {n} characters, more than Windows allows ({MAX_PATH_VALUE_LEN})."
            ),
            PathError::UnsupportedType(t) => {
                write!(f, "it's stored as {t}, not as a string value.")
            }
            PathError::EmbeddedNul => f.write_str("it contains a NUL character."),
            PathError::Unreadable(why) => write!(f, "it couldn't be read ({why})."),
        }
    }
}

/// What to do with the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    Unchanged,
    Set(String),
    /// The value would be empty: remove it.
    Delete,
}

/// Validate and tidy the folder argument (trailing separators dropped).
pub fn clean_dir(dir: &str) -> std::result::Result<String, PathError> {
    let d = dir.trim();
    if d.is_empty() {
        return Err(PathError::BadDir("is empty"));
    }
    if d.contains([';', '"', '\0', '%']) {
        return Err(PathError::BadDir("name contains ; \" or %"));
    }
    let bytes = d.as_bytes();
    let drive_abs = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/');
    let unc = d.starts_with(r"\\") && d.len() > 2;
    if !drive_abs && !unc {
        return Err(PathError::BadDir("isn't a full path"));
    }
    let trimmed = d.trim_end_matches(['\\', '/']);
    // Keep `C:\` as is: `C:` alone means "current folder on C".
    Ok(if trimmed.len() <= 2 {
        d.to_string()
    } else {
        trimmed.to_string()
    })
}

/// Split on `;` outside quotes. Joining the parts with `;` gives the input back.
pub fn split_entries(value: &str) -> std::result::Result<Vec<&str>, PathError> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    for (i, c) in value.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted => {
                parts.push(&value[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if quoted {
        return Err(PathError::UnbalancedQuotes);
    }
    parts.push(&value[start..]);
    Ok(parts)
}

/// Comparison form of a PATH entry: no quotes, `\` separators, no trailing
/// separator, lower case. `%VAR%` stays literal.
pub fn entry_key(entry: &str) -> String {
    let unquoted: String = entry.chars().filter(|c| *c != '"').collect();
    let slashed = unquoted.trim().replace('/', "\\");
    slashed.trim_end_matches('\\').to_lowercase()
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Append `dir` unless an entry already names it. Everything else is kept.
pub fn add_entry(current: Option<&str>, dir: &str) -> std::result::Result<Edit, PathError> {
    let value = current.unwrap_or("");
    if value.contains('\0') {
        return Err(PathError::EmbeddedNul);
    }
    let parts = split_entries(value)?;
    let want = entry_key(dir);
    if parts.iter().any(|p| entry_key(p) == want) {
        return Ok(Edit::Unchanged);
    }
    let new = if value.is_empty() {
        dir.to_string()
    } else if value.ends_with(';') {
        format!("{value}{dir}")
    } else {
        format!("{value};{dir}")
    };
    let len = utf16_len(&new);
    if len > MAX_PATH_VALUE_LEN {
        return Err(PathError::TooLong(len));
    }
    Ok(Edit::Set(new))
}

/// Drop every entry that names `dir`. Everything else is kept.
pub fn remove_entry(current: Option<&str>, dir: &str) -> std::result::Result<Edit, PathError> {
    let Some(value) = current else {
        return Ok(Edit::Unchanged);
    };
    if value.contains('\0') {
        return Err(PathError::EmbeddedNul);
    }
    let parts = split_entries(value)?;
    let want = entry_key(dir);
    let kept: Vec<&str> = parts
        .iter()
        .copied()
        .filter(|p| entry_key(p) != want)
        .collect();
    if kept.len() == parts.len() {
        return Ok(Edit::Unchanged);
    }
    let new = kept.join(";");
    Ok(if new.is_empty() {
        Edit::Delete
    } else {
        Edit::Set(new)
    })
}

// ---------------------------------------------------------------------------
// .reg file format (pure)
// ---------------------------------------------------------------------------

/// Registry string types GitHop will edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegKind {
    Sz,
    ExpandSz,
}

/// One registry value as read from / written to a `.reg` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegValue {
    /// Exact name as stored (case kept).
    pub name: String,
    pub kind: RegKind,
    pub data: String,
}

/// `HKCU\X` → `HKEY_CURRENT_USER\X` (the form `.reg` files use).
pub fn reg_file_key(key: &str) -> String {
    match key.split_once('\\') {
        Some((root, rest)) if root.eq_ignore_ascii_case("HKCU") => {
            format!("HKEY_CURRENT_USER\\{rest}")
        }
        _ => key.to_string(),
    }
}

/// Decode a `reg export` file: UTF-16LE with BOM.
pub fn decode_reg_file(bytes: &[u8]) -> std::result::Result<String, PathError> {
    let body = bytes
        .strip_prefix(&[0xFF, 0xFE])
        .ok_or_else(|| PathError::Unreadable("export isn't UTF-16".into()))?;
    if body.len() % 2 != 0 {
        return Err(PathError::Unreadable("export has an odd length".into()));
    }
    let units: Vec<u16> = body
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16(&units).map_err(|_| PathError::Unreadable("export isn't valid UTF-16".into()))
}

/// Find value `name` (case-insensitive) directly under `key` in a `.reg` text.
/// `Ok(None)`: the key section has no such value.
pub fn parse_reg_value(
    text: &str,
    key: &str,
    name: &str,
) -> std::result::Result<Option<RegValue>, PathError> {
    let header = format!("[{}]", reg_file_key(key));
    let mut lines = text.lines();
    let mut found_header = false;
    for line in lines.by_ref() {
        if line.trim_end().eq_ignore_ascii_case(&header) {
            found_header = true;
            break;
        }
    }
    if !found_header {
        return Err(PathError::Unreadable("key not in export".into()));
    }
    // Logical lines: hex data continues on the next line after a trailing `\`.
    let mut logical: Vec<String> = Vec::new();
    let mut pending: Option<String> = None;
    for line in lines {
        let line = line.trim_end_matches('\r');
        if pending.is_none() && line.starts_with('[') {
            break;
        }
        let mut piece = match pending.take() {
            Some(mut p) => {
                p.push_str(line.trim_start());
                p
            }
            None => line.to_string(),
        };
        if piece.ends_with('\\') && is_hex_line(&piece) {
            piece.pop();
            pending = Some(piece);
        } else {
            logical.push(piece);
        }
    }
    if pending.is_some() {
        return Err(PathError::Unreadable("export ends mid-value".into()));
    }
    for line in logical {
        if !line.starts_with('"') {
            continue;
        }
        let (value_name, rest) = parse_quoted(&line)?;
        if !value_name.eq_ignore_ascii_case(name) {
            continue;
        }
        let data = rest
            .strip_prefix('=')
            .ok_or_else(|| PathError::Unreadable("value line has no '='".into()))?;
        let (kind, data) = if data.starts_with('"') {
            let (s, tail) = parse_quoted(data)?;
            if !tail.trim().is_empty() {
                return Err(PathError::Unreadable("text after a string value".into()));
            }
            (RegKind::Sz, s)
        } else if let Some(hex) = strip_prefix_ci(data, "hex(2):") {
            (RegKind::ExpandSz, utf16_from_hex(hex)?)
        } else {
            let t = data.split(':').next().unwrap_or(data);
            return Err(PathError::UnsupportedType(type_name(t)));
        };
        if data.contains('\0') {
            return Err(PathError::EmbeddedNul);
        }
        return Ok(Some(RegValue {
            name: value_name,
            kind,
            data,
        }));
    }
    Ok(None)
}

/// A `"name"=hex…:` line (only those wrap with a trailing `\`).
fn is_hex_line(line: &str) -> bool {
    match parse_quoted(line) {
        Ok((_, rest)) => rest
            .strip_prefix('=')
            .is_some_and(|d| strip_prefix_ci(d, "hex").is_some()),
        // A continuation of a hex line (`  00,4a,…`) has no quote at the start.
        Err(_) => !line.starts_with('"'),
    }
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

fn type_name(tag: &str) -> String {
    match tag.to_ascii_lowercase().as_str() {
        "dword" => "REG_DWORD".into(),
        "hex" => "REG_BINARY".into(),
        "hex(7)" => "REG_MULTI_SZ".into(),
        "hex(b)" => "REG_QWORD".into(),
        "hex(0)" => "REG_NONE".into(),
        "hex(1)" => "REG_SZ (hex)".into(),
        "-" => "a deleted value".into(),
        other => format!("an unknown type ({other})"),
    }
}

/// Parse `"…"` at the start of `s` (`\\` and `\"` escapes). Returns the text and the rest.
fn parse_quoted(s: &str) -> std::result::Result<(String, &str), PathError> {
    let body = s
        .strip_prefix('"')
        .ok_or_else(|| PathError::Unreadable("expected a quote".into()))?;
    let mut out = String::new();
    let mut chars = body.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Ok((out, &body[i + 1..])),
            '\\' => match chars.next() {
                Some((_, '\\')) => out.push('\\'),
                Some((_, '"')) => out.push('"'),
                _ => return Err(PathError::Unreadable("unknown escape in a string".into())),
            },
            c => out.push(c),
        }
    }
    Err(PathError::Unreadable("unterminated string".into()))
}

/// `43,00,3a,00,00,00` → `C:` (UTF-16LE; trailing NULs dropped).
fn utf16_from_hex(hex: &str) -> std::result::Result<String, PathError> {
    let mut bytes = Vec::new();
    for part in hex.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        let b = u8::from_str_radix(p, 16)
            .map_err(|_| PathError::Unreadable("bad hex in export".into()))?;
        bytes.push(b);
    }
    if bytes.len() % 2 != 0 {
        return Err(PathError::Unreadable("odd number of bytes".into()));
    }
    let mut units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    while units.last() == Some(&0) {
        units.pop();
    }
    String::from_utf16(&units).map_err(|_| PathError::Unreadable("value isn't valid UTF-16".into()))
}

fn escape_reg_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// A `.reg` file (text; encode with [`encode_reg_file`]) that sets one value.
pub fn render_reg_file(key: &str, value: &RegValue) -> String {
    let mut out = String::from("Windows Registry Editor Version 5.00\r\n\r\n");
    out.push_str(&format!("[{}]\r\n", reg_file_key(key)));
    let name = format!("\"{}\"=", escape_reg_string(&value.name));
    match value.kind {
        RegKind::Sz => {
            out.push_str(&name);
            out.push('"');
            out.push_str(&escape_reg_string(&value.data));
            out.push('"');
        }
        RegKind::ExpandSz => {
            out.push_str(&name);
            out.push_str("hex(2):");
            let mut bytes: Vec<u8> = value
                .data
                .encode_utf16()
                .flat_map(|u| u.to_le_bytes())
                .collect();
            bytes.extend([0, 0]);
            for (i, b) in bytes.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                    // Wrap like regedit does, so lines stay short.
                    if i % 24 == 0 {
                        out.push_str("\\\r\n  ");
                    }
                }
                out.push_str(&format!("{b:02x}"));
            }
        }
    }
    out.push_str("\r\n\r\n");
    out
}

/// UTF-16LE with BOM, as `reg import` expects for version 5 files.
pub fn encode_reg_file(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(text.encode_utf16().flat_map(|u| u.to_le_bytes()));
    bytes
}

// ---------------------------------------------------------------------------
// reg.exe
// ---------------------------------------------------------------------------

/// A fresh private folder for `.reg` files; removed on drop.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new() -> Result<Self> {
        let base = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        for n in 0..16u32 {
            let dir = base.join(format!(
                "githop-path-{}-{nanos}-{n}",
                std::process::id()
            ));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok(Self(dir)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(anyhow!("couldn't create a temporary folder"))
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn reg_exe() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(|r| PathBuf::from(r).join("System32").join("reg.exe"))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("reg.exe"))
}

/// Run `reg.exe` with an argument array, no window, no output. Exit code.
fn run_reg(args: &[&std::ffi::OsStr]) -> std::io::Result<i32> {
    let mut cmd = Command::new(reg_exe());
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(cmd.status()?.code().unwrap_or(-1))
}

fn os(s: &str) -> &std::ffi::OsStr {
    std::ffi::OsStr::new(s)
}

/// Read `Path` under `key`. `Ok(None)`: no key or no value.
fn read_value(key: &str) -> std::result::Result<Option<RegValue>, PathError> {
    let io = |e: std::io::Error| PathError::Unreadable(format!("reg.exe: {e}"));
    match run_reg(&[os("query"), os(key)]).map_err(io)? {
        0 => {}
        1 => return Ok(None),
        code => return Err(PathError::Unreadable(format!("reg query exit {code}"))),
    }
    let scratch = ScratchDir::new().map_err(|e| PathError::Unreadable(e.to_string()))?;
    let file = scratch.file("export.reg");
    let code = run_reg(&[os("export"), os(key), file.as_os_str(), os("/y")]).map_err(io)?;
    if code != 0 {
        return Err(PathError::Unreadable(format!("reg export exit {code}")));
    }
    let bytes = std::fs::read(&file).map_err(io)?;
    let text = decode_reg_file(&bytes)?;
    parse_reg_value(&text, key, PATH_VALUE)
}

fn import(key: &str, value: &RegValue) -> Result<()> {
    let scratch = ScratchDir::new()?;
    let file = scratch.file("import.reg");
    write_new(&file, &encode_reg_file(&render_reg_file(key, value)))?;
    let code = run_reg(&[os("import"), file.as_os_str()])?;
    if code != 0 {
        return Err(anyhow!("reg import exit {code}"));
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}

/// Put the old value back after a failed write (best effort).
fn restore(key: &str, old: Option<&RegValue>) {
    match old {
        Some(v) => {
            let _ = import(key, v);
        }
        None => {
            let _ = run_reg(&[os("delete"), os(key), os("/v"), os(PATH_VALUE), os("/f")]);
        }
    }
}

fn write_verified(key: &str, new: &RegValue, old: Option<&RegValue>) -> Result<()> {
    let result = import(key, new).and_then(|()| match read_value(key) {
        Ok(Some(back)) if back.kind == new.kind && back.data == new.data => Ok(()),
        Ok(_) => Err(anyhow!("the value read back doesn't match")),
        Err(e) => Err(anyhow!("{e}")),
    });
    if let Err(e) = result {
        restore(key, old);
        return Err(anyhow!(
            "GitHop couldn't update your PATH ({e}). It was left as it was."
        ));
    }
    Ok(())
}

fn delete_verified(key: &str, name: &str, old: Option<&RegValue>) -> Result<()> {
    let code = run_reg(&[os("delete"), os(key), os("/v"), os(name), os("/f")])?;
    let gone = code == 0 && matches!(read_value(key), Ok(None));
    if !gone {
        restore(key, old);
        return Err(anyhow!(
            "GitHop couldn't update your PATH (reg delete exit {code}). It was left as it was."
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = r"C:\Users\Me\AppData\Local\GitHop";

    fn set(e: Edit) -> String {
        match e {
            Edit::Set(s) => s,
            other => panic!("expected Set, got {other:?}"),
        }
    }

    #[test]
    fn add_appends_once_and_keeps_everything_else() {
        assert_eq!(set(add_entry(None, DIR).unwrap()), DIR);
        assert_eq!(set(add_entry(Some(""), DIR).unwrap()), DIR);
        let v = r"%USERPROFILE%\bin;;C:\Tools;";
        assert_eq!(set(add_entry(Some(v), DIR).unwrap()), format!("{v}{DIR}"));
        let v = r"%USERPROFILE%\bin;C:\Tools";
        assert_eq!(set(add_entry(Some(v), DIR).unwrap()), format!("{v};{DIR}"));
        // Already there in any spelling → unchanged.
        for there in [
            format!("A;{DIR};B"),
            format!("A;{}\\", DIR.to_uppercase()),
            format!("\"{DIR}\";B"),
            format!("{};B", DIR.replace('\\', "/")),
        ] {
            assert_eq!(add_entry(Some(&there), DIR).unwrap(), Edit::Unchanged, "{there}");
        }
    }

    #[test]
    fn remove_drops_every_copy_and_keeps_the_rest_exactly() {
        let v = format!(r"%SystemRoot%\x;;{DIR};C:\Tools;{}\;", DIR.to_lowercase());
        assert_eq!(
            set(remove_entry(Some(&v), DIR).unwrap()),
            r"%SystemRoot%\x;;C:\Tools;"
        );
        assert_eq!(remove_entry(Some(DIR), DIR).unwrap(), Edit::Delete);
        assert_eq!(remove_entry(Some(&format!("{DIR};")), DIR).unwrap(), Edit::Delete);
        assert_eq!(remove_entry(Some("A;B"), DIR).unwrap(), Edit::Unchanged);
        assert_eq!(remove_entry(None, DIR).unwrap(), Edit::Unchanged);
        // Quoted entries with ; inside stay whole.
        let v = format!("\"C:\\a;b\";{DIR}");
        assert_eq!(set(remove_entry(Some(&v), DIR).unwrap()), "\"C:\\a;b\"");
    }

    #[test]
    fn similar_folders_are_not_touched() {
        let v = format!(r"{DIR}2;{DIR}\bin;%LOCALAPPDATA%\GitHop");
        assert_eq!(remove_entry(Some(&v), DIR).unwrap(), Edit::Unchanged);
        assert_eq!(set(add_entry(Some(&v), DIR).unwrap()), format!("{v};{DIR}"));
    }

    #[test]
    fn long_values_are_kept_whole() {
        // Past the old 1023-char NSIS limit, and right around it.
        for n in [990, 1000, 1023, 1024, 1109, 8000, 30000] {
            let mut v = String::new();
            let mut i = 0;
            while v.len() < n {
                v.push_str(&format!(r"C:\dir{i}\bin;"));
                i += 1;
            }
            v.truncate(n);
            let v = v.trim_end_matches(';').to_string();
            let added = set(add_entry(Some(&v), DIR).unwrap());
            assert!(added.starts_with(&v) && added.ends_with(DIR));
            assert_eq!(added.len(), v.len() + 1 + DIR.len());
            assert_eq!(set(remove_entry(Some(&added), DIR).unwrap()), v);
        }
    }

    #[test]
    fn refuses_rather_than_guessing() {
        assert_eq!(
            add_entry(Some("\"C:\\a;B"), DIR),
            Err(PathError::UnbalancedQuotes)
        );
        assert_eq!(
            remove_entry(Some("\"C:\\a;B"), DIR),
            Err(PathError::UnbalancedQuotes)
        );
        assert_eq!(add_entry(Some("a\0b"), DIR), Err(PathError::EmbeddedNul));
        let huge = "x".repeat(MAX_PATH_VALUE_LEN - DIR.len());
        assert!(matches!(
            add_entry(Some(&huge), DIR),
            Err(PathError::TooLong(_))
        ));
        let fits = "x".repeat(MAX_PATH_VALUE_LEN - DIR.len() - 1);
        assert!(add_entry(Some(&fits), DIR).is_ok());
    }

    #[test]
    fn dir_argument_rules() {
        assert_eq!(clean_dir(&format!("{DIR}\\")).unwrap(), DIR);
        assert_eq!(clean_dir(r"  C:\Git Hop dir  ").unwrap(), r"C:\Git Hop dir");
        assert_eq!(clean_dir(r"C:\").unwrap(), r"C:\");
        assert!(clean_dir(r"\\server\share\GitHop").is_ok());
        for bad in ["", "GitHop", r"C:GitHop", r"C:\a;b", "C:\\a\"", r"%LOCALAPPDATA%\GitHop"] {
            assert!(clean_dir(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn test_key_override_rules() {
        assert!(is_test_key(r"HKCU\Software\GitHopTest-123"));
        assert!(is_test_key(r"hkcu\software\githoptest-1\env"));
        assert!(!is_test_key(r"HKCU\Software\GitHopTest"));
        assert!(!is_test_key(r"HKCU\Environment"));
        assert!(!is_test_key(r"HKCU\Software\GitHopTest\..\..\Environment"));
        assert!(!is_test_key(r#"HKCU\Software\GitHopTest-1" /f"#));
    }

    #[test]
    fn reg_file_round_trip() {
        let key = r"HKCU\Software\GitHopTest-1";
        for kind in [RegKind::Sz, RegKind::ExpandSz] {
            let v = RegValue {
                name: "PATH".into(),
                kind,
                data: format!("C:\\Jös\\é€;%USERPROFILE%\\bin;\"C:\\q;x\";{}", "a".repeat(300)),
            };
            let text = render_reg_file(key, &v);
            let bytes = encode_reg_file(&text);
            let back = decode_reg_file(&bytes).unwrap();
            assert_eq!(parse_reg_value(&back, key, "Path").unwrap(), Some(v));
        }
    }

    #[test]
    fn parses_a_real_export() {
        // As written by `reg export` (wrapped hex, other values, a subkey).
        let text = "Windows Registry Editor Version 5.00\r\n\r\n\
            [HKEY_CURRENT_USER\\Environment]\r\n\
            \"TEMP\"=hex(2):25,00,55,00,00,00\r\n\
            \"Path\"=hex(2):43,00,3a,00,5c,00,4a,00,f6,00,73,00,3b,00,25,00,55,00,53,00,\\\r\n  \
            45,00,52,00,25,00,00,00\r\n\
            \"Other\"=\"x\\\\y\"\r\n\r\n\
            [HKEY_CURRENT_USER\\Environment\\Sub]\r\n\
            \"Path\"=\"nope\"\r\n";
        let v = parse_reg_value(text, r"HKCU\Environment", "path")
            .unwrap()
            .unwrap();
        assert_eq!(v.kind, RegKind::ExpandSz);
        assert_eq!(v.name, "Path");
        assert_eq!(v.data, "C:\\Jös;%USER%");
        assert_eq!(
            parse_reg_value(text, r"HKCU\Environment", "Missing").unwrap(),
            None
        );
        let sz = "[HKEY_CURRENT_USER\\Environment]\r\n\"Path\"=\"C:\\\\a;C:\\\\b\\\\\"\r\n";
        let v = parse_reg_value(sz, r"HKCU\Environment", "Path")
            .unwrap()
            .unwrap();
        assert_eq!(v.kind, RegKind::Sz);
        assert_eq!(v.data, r"C:\a;C:\b\");
        let multi = "[HKEY_CURRENT_USER\\Environment]\r\n\"Path\"=hex(7):41,00,00,00,00,00\r\n";
        assert!(matches!(
            parse_reg_value(multi, r"HKCU\Environment", "Path"),
            Err(PathError::UnsupportedType(t)) if t == "REG_MULTI_SZ"
        ));
        assert!(parse_reg_value("[HKEY_CURRENT_USER\\Other]\r\n", r"HKCU\Environment", "Path").is_err());
    }
}
