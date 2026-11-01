//! `githop installer path-add|path-remove` against a throwaway registry key
//! (`HKCU\Software\GitHopTest-<pid>-<n>`, via `GITHOP_PATH_KEY`). The real
//! `HKCU\Environment` is never read or written here.
//!
//! The registry is set up and checked with .NET's registry API through
//! PowerShell (an independent reader that keeps `%VAR%` unexpanded and reports
//! the value kind), with every value passed through a UTF-8 file, never on a
//! command line.

#![cfg(windows)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use support::TestEnv;

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A test key, deleted on drop.
struct TestKey {
    /// `Software\GitHopTest-…` (under HKCU).
    sub: String,
}

impl TestKey {
    fn new() -> Self {
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        Self {
            sub: format!(r"Software\GitHopTest-{}-{n}", std::process::id()),
        }
    }

    fn reg_name(&self) -> String {
        format!(r"HKCU\{}", self.sub)
    }
}

impl Drop for TestKey {
    fn drop(&mut self) {
        let _ = Command::new("reg")
            .args(["delete", &self.reg_name(), "/f"])
            .output();
    }
}

/// Run a PowerShell script with `GH_KEY` and `GH_FILE` set.
fn ps(script: &str, key: &TestKey, file: &Path) -> String {
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("GH_KEY", &key.sub)
        .env("GH_FILE", file)
        .output()
        .expect("run powershell");
    assert!(
        out.status.success(),
        "powershell failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Set `Path` under the test key to `value` with kind `ExpandString` or `String`.
fn set_value(env: &TestEnv, key: &TestKey, value: &str, kind: &str) {
    let file = env.root().join("in.txt");
    std::fs::write(&file, value.as_bytes()).unwrap();
    let script = format!(
        "$v = [IO.File]::ReadAllText($env:GH_FILE, [Text.UTF8Encoding]::new($false)); \
         $k = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($env:GH_KEY); \
         $k.SetValue('Path', $v, [Microsoft.Win32.RegistryValueKind]::{kind}); $k.Close()"
    );
    ps(&script, key, &file);
}

/// `(kind, value)` of `Path` under the test key, unexpanded; `None` if absent.
fn get_value(env: &TestEnv, key: &TestKey) -> Option<(String, String)> {
    let file = env.root().join("out.txt");
    let _ = std::fs::remove_file(&file);
    let script = "$k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($env:GH_KEY); \
         if ($k -eq $null) { 'nokey'; exit 0 }; \
         $v = $k.GetValue('Path', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames); \
         if ($v -eq $null) { 'novalue'; exit 0 }; \
         [IO.File]::WriteAllText($env:GH_FILE, $v, [Text.UTF8Encoding]::new($false)); \
         $k.GetValueKind('Path').ToString()";
    let kind = ps(script, key, &file);
    if kind == "nokey" || kind == "novalue" {
        return None;
    }
    let value = std::fs::read_to_string(&file).unwrap();
    Some((kind, value))
}

/// Run `githop installer <args>` against the test key. `(code, stdout+stderr)`.
fn githop(env: &TestEnv, key: &TestKey, args: &[&str]) -> (i32, String) {
    let out = env
        .githop()
        .arg("installer")
        .args(args)
        .env("GITHOP_PATH_KEY", key.reg_name())
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.code().unwrap_or(-1), text)
}

fn long_path(len: usize) -> String {
    let mut v = String::from(r"%USERPROFILE%\.cargo\bin;C:\Jös\é€ tools;;");
    let mut i = 0;
    while v.len() < len {
        v.push_str(&format!(r"C:\Program Files\Tool {i}\bin;"));
        i += 1;
    }
    v.truncate(len);
    v
}

const DIR: &str = r"C:\Users\Test User\AppData\Local\GitHop";

#[test]
fn long_paths_survive_add_and_remove_exactly() {
    let env = TestEnv::new();
    // Over the old 1023-char NSIS limit, near it, and very long.
    for (len, kind) in [
        (1109, "ExpandString"),
        (998, "ExpandString"),
        (1000, "String"),
        (20_000, "ExpandString"),
    ] {
        let key = TestKey::new();
        let original = long_path(len);
        set_value(&env, &key, &original, kind);

        let (code, out) = githop(&env, &key, &["path-add", DIR]);
        assert_eq!(code, 0, "{out}");
        let (k, v) = get_value(&env, &key).unwrap();
        assert_eq!(k, kind, "type kept");
        let expected = if original.ends_with(';') {
            format!("{original}{DIR};")
        } else {
            format!("{original};{DIR}")
        };
        assert_eq!(v, expected, "len {len}: everything kept, dir appended once");

        // Again: no duplicate.
        let (code, out) = githop(&env, &key, &["path-add", &format!("{DIR}\\")]);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("already on your PATH"), "{out}");
        assert_eq!(get_value(&env, &key).unwrap().1, expected);

        let (code, out) = githop(&env, &key, &["path-remove", &DIR.to_uppercase()]);
        assert_eq!(code, 0, "{out}");
        assert_eq!(
            get_value(&env, &key).unwrap(),
            (kind.to_string(), original.clone()),
            "len {len}: back to the original, byte for byte"
        );
    }
}

#[test]
fn missing_value_is_created_and_removed() {
    let env = TestEnv::new();
    let key = TestKey::new();
    let (code, out) = githop(&env, &key, &["path-add", DIR]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(
        get_value(&env, &key).unwrap(),
        ("ExpandString".to_string(), DIR.to_string())
    );
    let (code, out) = githop(&env, &key, &["path-remove", DIR]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(get_value(&env, &key), None, "value removed when empty");
    let (code, out) = githop(&env, &key, &["path-remove", DIR]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("wasn't on your PATH"), "{out}");
}

#[test]
fn ambiguous_values_are_left_alone() {
    let env = TestEnv::new();
    let key = TestKey::new();
    let original = r#"C:\a;"C:\unclosed;C:\b"#;
    set_value(&env, &key, original, "ExpandString");
    let (code, out) = githop(&env, &key, &["path-add", DIR]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("PATH wasn't changed"), "{out}");
    assert_eq!(get_value(&env, &key).unwrap().1, original);

    // A multi-string Path isn't touched either.
    let multi = TestKey::new();
    let file = env.root().join("m.txt");
    ps(
        "$k = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($env:GH_KEY); \
         $k.SetValue('Path', [string[]]@('C:\\a','C:\\b'), [Microsoft.Win32.RegistryValueKind]::MultiString); $k.Close()",
        &multi,
        &file,
    );
    let (code, out) = githop(&env, &multi, &["path-add", DIR]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("REG_MULTI_SZ"), "{out}");
}

#[test]
fn only_test_keys_are_accepted_from_the_override() {
    let env = TestEnv::new();
    for bad in [
        r"HKCU\Environment",
        r"HKCU\Software\GitHopTest\..\..\Environment",
    ] {
        let out = env
            .githop()
            .args(["installer", "path-add", DIR])
            .env("GITHOP_PATH_KEY", bad)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{bad}");
    }
    // No override + GITHOP_NO_REGISTRY=1 (set by the test env): nothing is touched.
    let out = env
        .githop()
        .args(["installer", "path-add", DIR])
        .env_remove("GITHOP_PATH_KEY")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Nothing was changed"));
}

#[test]
fn hooks_install_if_installed_does_nothing_on_a_fresh_pc() {
    let env = TestEnv::new();
    let data = env.root().join("fresh-data");
    let out = env
        .githop()
        .args(["hooks", "install", "--yes", "--if-installed"])
        .env("GITHOP_HOME", &data)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("nothing to update"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(!data.exists(), "no data folder created");
    let cfg = std::fs::read_to_string(env.global_config()).unwrap();
    assert!(!cfg.contains("hooksPath"), "{cfg}");
}

#[test]
fn hooks_install_if_installed_rewrites_installed_hooks() {
    let env = TestEnv::new();
    env.githop()
        .args(["hooks", "install", "--yes"])
        .assert()
        .code(0);
    let out = env
        .githop()
        .args(["hooks", "install", "--if-installed"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Hooks are up to date"), "{text}");
}
