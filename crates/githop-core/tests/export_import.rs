//! Encrypted export / import (Phase 6 §3) against isolated `TestEnv`s — two
//! separate temp "machines", never the real `~/.ssh` or `%APPDATA%` (CLAUDE.md rule 3).
//!
//! Most tests encrypt with a tiny scrypt work factor so they stay fast; one test
//! uses the real export work factor end to end.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::path::{Path, PathBuf};

use githop_core::export::{
    self, apply_bundle, decrypt_bytes, encrypt_bytes, export_with_work_factor, open_bundle,
    parse_bundle, preview_bundle, ConflictKind, ExportError, ImportAction, ImportResolution,
    MAX_BUNDLE_BYTES,
};
use githop_core::{EnforceMode, Profile, ProfileColor, ProfileSet, Settings, Store};
use support::TestEnv;

const PASSWORD: &str = "correct horse battery staple";
/// Fast scrypt for tests (the real export uses 2^18).
const FAST: u8 = 2;

const OFFICE_EMAIL: &str = "me@company.com";
const PERSONAL_EMAIL: &str = "12345+me@users.noreply.github.com";

fn ssh_dir(env: &TestEnv) -> PathBuf {
    env.home().join(".ssh")
}

fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn private_key_text(tag: &str) -> String {
    format!(
        "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA{tag}\n-----END OPENSSH PRIVATE KEY-----\n"
    )
}

/// Placeholder key pair (not a real key) at `~/.ssh/<name>`.
fn write_key(env: &TestEnv, name: &str, tag: &str) -> PathBuf {
    std::fs::create_dir_all(ssh_dir(env)).unwrap();
    let key = ssh_dir(env).join(name);
    std::fs::write(&key, private_key_text(tag)).unwrap();
    std::fs::write(
        ssh_dir(env).join(format!("{name}.pub")),
        format!("ssh-ed25519 AAAAC3Nz{tag} {name}\n"),
    )
    .unwrap();
    key
}

fn profile(env: &TestEnv, id: &str, email: &str, color: ProfileColor, owner: &str) -> Profile {
    Profile {
        id: id.into(),
        label: format!("{}{}", id[..1].to_uppercase(), &id[1..]),
        name: format!("Me ({id})"),
        email: email.into(),
        color,
        ssh_key: PathBuf::from(slash(&ssh_dir(env).join(format!("githop_{id}")))),
        owners: vec![owner.into()],
        folders: vec![format!("C:/{id} work/")],
        sign_commits: false,
    }
}

/// "Machine A": Office + Personal, with keys, warn mode, pins and hooks state.
fn source_machine() -> TestEnv {
    let env = TestEnv::new();
    write_key(&env, "githop_office", "office");
    write_key(&env, "githop_personal", "personal");
    let set = ProfileSet {
        version: 1,
        default: Some("personal".into()),
        profiles: vec![
            profile(
                &env,
                "office",
                OFFICE_EMAIL,
                ProfileColor::Blue,
                "office-org",
            ),
            profile(
                &env,
                "personal",
                PERSONAL_EMAIL,
                ProfileColor::Violet,
                "me-personal",
            ),
        ],
    };
    let settings = Settings {
        enforce: EnforceMode::Warn,
        logging: true,
        log_retention_days: 3,
        hooks_installed: true,
        previous_hooks_path: "C:/secret/hooks".into(),
        pinned_repos: vec!["C:/private/repo/".into()],
        ..Settings::default()
    };
    Store::open_at(env.paths())
        .unwrap()
        .save(&set, &settings)
        .unwrap();
    env
}

fn export_file(env: &TestEnv, include_keys: bool) -> PathBuf {
    let out = env.root().join("my profiles.githop");
    export_with_work_factor(
        &env.paths(),
        &env.git_handle(),
        &out,
        PASSWORD,
        include_keys,
        FAST,
    )
    .unwrap();
    out
}

fn store(env: &TestEnv) -> Store {
    Store::open_at(env.paths()).unwrap()
}

fn resolve(id: &str, action: ImportAction, new_id: Option<&str>) -> ImportResolution {
    ImportResolution {
        profile_id: id.into(),
        action,
        new_id: new_id.map(str::to_string),
    }
}

/// A valid plaintext bundle, for building hostile variants.
fn bundle_text(key_name: &str, key_entry_name: &str) -> String {
    use base64::Engine as _;
    let private = base64::engine::general_purpose::STANDARD.encode(private_key_text("x"));
    format!(
        r#"format = "githop-export"
version = 1
created = "2026-10-31T10:00:00Z"

[profiles]
version = 1
default = "office"

[[profiles.profile]]
id = "office"
label = "Office"
name = "Me"
email = "me@company.com"
color = "blue"
ssh_key = "{key_name}"
owners = ["office-org"]
folders = ["C:/Office/"]

[settings]
enforce = "block"
logging = false
log_retention_days = 7

[[keys]]
name = "{key_entry_name}"
private = "{private}"
public = "ssh-ed25519 AAAA test"
"#
    )
}

// ---------------------------------------------------------------- round trips

#[test]
fn round_trip_without_keys() {
    let a = source_machine();
    let file = export_file(&a, false);
    let b = TestEnv::new();

    let bundle = open_bundle(&file, PASSWORD).unwrap();
    assert!(bundle.key_names().is_empty());
    let preview = preview_bundle(&b.paths(), &b.git_handle(), &bundle).unwrap();
    assert!(preview.conflicts.is_empty(), "{:?}", preview.conflicts);
    assert!(preview.include_settings);
    assert_eq!(preview.profiles.len(), 2);
    let expected_key = slash(&ssh_dir(&b).join("githop_office"));
    assert_eq!(slash(&preview.profiles[0].ssh_key), expected_key);

    let report = apply_bundle(&b.paths(), &b.git_handle(), &bundle, &[]).unwrap();
    assert_eq!(report.imported, ["office", "personal"]);
    assert!(report.keys_written.is_empty());
    assert_eq!(report.missing_keys.len(), 2);
    assert!(report.settings_applied);
    assert!(report.default_changed);

    let s = store(&b);
    let src = store(&a);
    assert_eq!(s.profiles().default.as_deref(), Some("personal"));
    for (got, want) in s.profiles().profiles.iter().zip(&src.profiles().profiles) {
        assert_eq!(got.id, want.id);
        assert_eq!(got.email, want.email);
        assert_eq!(got.owners, want.owners);
        assert_eq!(got.folders, want.folders);
        assert_eq!(got.color, want.color);
        // Key path rewritten to *this* machine's ~/.ssh.
        assert_eq!(
            slash(&got.ssh_key),
            slash(&ssh_dir(&b).join(format!("githop_{}", got.id)))
        );
    }
    // Settings subset only; machine-specific state never travels.
    assert_eq!(s.settings().enforce, EnforceMode::Warn);
    assert!(s.settings().logging);
    assert_eq!(s.settings().log_retention_days, 3);
    assert!(!s.settings().hooks_installed);
    assert!(s.settings().pinned_repos.is_empty());
    assert!(s.settings().previous_hooks_path.is_empty());
    assert!(!ssh_dir(&b).exists());
}

#[test]
fn round_trip_with_keys_sets_acl() {
    let a = source_machine();
    let file = export_file(&a, true);
    let b = TestEnv::new();

    let bundle = open_bundle(&file, PASSWORD).unwrap();
    assert_eq!(bundle.key_names(), ["githop_office", "githop_personal"]);
    let report = apply_bundle(&b.paths(), &b.git_handle(), &bundle, &[]).unwrap();
    assert_eq!(report.keys_written.len(), 2);
    assert!(report.missing_keys.is_empty());

    for name in ["githop_office", "githop_personal"] {
        let src = std::fs::read(ssh_dir(&a).join(name)).unwrap();
        let dst = ssh_dir(&b).join(name);
        assert_eq!(std::fs::read(&dst).unwrap(), src);
        let public = std::fs::read_to_string(ssh_dir(&b).join(format!("{name}.pub"))).unwrap();
        assert!(public.starts_with("ssh-ed25519 "), "{public}");
        assert_user_only(&dst);
    }

    // Importing the same file again: identical keys are no conflict, ids are.
    let bundle = open_bundle(&file, PASSWORD).unwrap();
    let preview = preview_bundle(&b.paths(), &b.git_handle(), &bundle).unwrap();
    assert!(preview
        .conflicts
        .iter()
        .all(|c| c.kind != ConflictKind::KeyFile));
    assert!(!preview.include_settings);
}

#[cfg(windows)]
fn assert_user_only(key: &Path) {
    let out = std::process::Command::new("icacls")
        .arg(key)
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    let user = std::env::var("USERNAME").unwrap();
    let aces: Vec<&str> = text.lines().filter(|l| l.contains(":(")).collect();
    assert_eq!(aces.len(), 1, "expected one ACE:\n{text}");
    assert!(aces[0].contains(&format!("{user}:(F)")), "{text}");
}

#[cfg(unix)]
fn assert_user_only(key: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(key).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn real_work_factor_round_trip() {
    let a = source_machine();
    let out = a.root().join("real.githop");
    let summary = export::export(&a.paths(), &a.git_handle(), &out, PASSWORD, true).unwrap();
    assert_eq!(summary.profiles, ["office", "personal"]);
    assert_eq!(summary.keys.len(), 2);
    let b = TestEnv::new();
    let set = export::apply(&b.paths(), &b.git_handle(), &out, PASSWORD, &[]).unwrap();
    assert_eq!(set.profiles.len(), 2);
    let preview = export::preview(&b.paths(), &b.git_handle(), &out, PASSWORD).unwrap();
    assert_eq!(preview.keys, ["githop_office", "githop_personal"]);
}

#[test]
fn export_never_contains_machine_paths() {
    let a = source_machine();
    let file = export_file(&a, false);
    let bytes = std::fs::read(&file).unwrap();
    assert!(bytes.starts_with(b"age-encryption.org/v1"));
    let plain = decrypt_bytes(&bytes, PASSWORD).unwrap();
    let text = String::from_utf8(plain.to_vec()).unwrap();
    assert!(text.contains("format = \"githop-export\""), "{text}");
    assert!(text.contains("ssh_key = \"githop_office\""), "{text}");
    assert!(!text.contains(&slash(&a.home())), "{text}");
    for secret in [
        "C:/secret/hooks",
        "C:/private/repo",
        "pinned",
        "hooks_installed",
    ] {
        assert!(!text.contains(secret), "{secret} leaked:\n{text}");
    }
    assert!(!text.contains("[[keys]]"));
}

#[test]
fn export_rejects_short_password_and_empty_store() {
    let a = source_machine();
    let out = a.root().join("x.githop");
    let r = export_with_work_factor(&a.paths(), &a.git_handle(), &out, "short pass", false, FAST);
    assert!(
        matches!(r, Err(ExportError::WeakPassword { min: 12 })),
        "{r:?}"
    );
    assert!(!out.exists());

    let empty = TestEnv::new();
    let r = export_with_work_factor(
        &empty.paths(),
        &empty.git_handle(),
        &out,
        PASSWORD,
        false,
        FAST,
    );
    assert!(matches!(r, Err(ExportError::NothingToExport)), "{r:?}");
}

#[test]
fn export_with_non_githop_key_name_uses_githop_id() {
    let a = TestEnv::new();
    let key = write_key(&a, "id_ed25519", "custom");
    let mut p = profile(&a, "office", OFFICE_EMAIL, ProfileColor::Blue, "office-org");
    p.ssh_key = PathBuf::from(slash(&key));
    let set = ProfileSet {
        version: 1,
        default: Some("office".into()),
        profiles: vec![p],
    };
    store(&a).save(&set, &Settings::default()).unwrap();
    let file = export_file(&a, true);
    let bundle = open_bundle(&file, PASSWORD).unwrap();
    assert_eq!(bundle.key_names(), ["githop_office"]);

    let b = TestEnv::new();
    apply_bundle(&b.paths(), &b.git_handle(), &bundle, &[]).unwrap();
    assert_eq!(
        std::fs::read(ssh_dir(&b).join("githop_office")).unwrap(),
        std::fs::read(&key).unwrap()
    );
}

// ---------------------------------------------------------------- bad files

#[test]
fn wrong_password_is_clear_error() {
    let a = source_machine();
    let file = export_file(&a, true);
    let r = open_bundle(&file, "not the right password");
    assert!(matches!(r, Err(ExportError::WrongPassword)), "{r:?}");
    let msg = ExportError::WrongPassword.to_string();
    assert!(msg.contains("password is wrong"), "{msg}");
    let r = open_bundle(&file, "");
    assert!(matches!(r, Err(ExportError::WrongPassword)), "{r:?}");
}

#[test]
fn tampered_ciphertext_is_rejected() {
    let a = source_machine();
    let file = export_file(&a, true);
    let good = std::fs::read(&file).unwrap();
    let positions = [
        0,
        10,
        good.len() / 4,
        good.len() / 2,
        good.len() - 20,
        good.len() - 1,
    ];
    for &i in &positions {
        let mut bad = good.clone();
        bad[i] ^= 0x01;
        match decrypt_bytes(&bad, PASSWORD) {
            Err(ExportError::Corrupt) | Err(ExportError::WrongPassword) => {}
            other => panic!("flip at {i}: expected rejection, got {other:?}"),
        }
    }
    // Truncated, extended, empty.
    for bad in [
        good[..good.len() - 1].to_vec(),
        good[..good.len() / 2].to_vec(),
        [good.clone(), b"extra".to_vec()].concat(),
        Vec::new(),
    ] {
        assert!(decrypt_bytes(&bad, PASSWORD).is_err());
    }
}

#[test]
fn oversized_file_is_rejected_before_decrypting() {
    let env = TestEnv::new();
    let big = env.root().join("big.githop");
    std::fs::write(&big, vec![b'a'; MAX_BUNDLE_BYTES as usize + 1]).unwrap();
    assert!(matches!(
        open_bundle(&big, PASSWORD),
        Err(ExportError::TooLarge { .. })
    ));
    let plain = vec![b' '; MAX_BUNDLE_BYTES as usize + 1];
    assert!(matches!(
        parse_bundle(&plain),
        Err(ExportError::TooLarge { .. })
    ));
}

#[test]
fn missing_file_is_io_error() {
    let env = TestEnv::new();
    let r = open_bundle(&env.root().join("nope.githop"), PASSWORD);
    assert!(matches!(r, Err(ExportError::Core(_))), "{r:?}");
}

#[test]
fn schema_is_strict() {
    let good = bundle_text("githop_office", "githop_office");
    assert!(parse_bundle(good.as_bytes()).is_ok());

    let cases = [
        // Unknown fields at every level.
        good.replace("version = 1\ncreated", "version = 1\nsurprise = 1\ncreated"),
        good.replace("color = \"blue\"", "color = \"blue\"\nextra = \"x\""),
        good.replace(
            "log_retention_days = 7",
            "log_retention_days = 7\nhooks_installed = true",
        ),
        good.replace("public = ", "comment = \"x\"\npublic = "),
        // Machine-specific settings never accepted.
        good.replace(
            "logging = false",
            "logging = false\npinned_repos = [\"C:/x/\"]",
        ),
        // Wrong format / types / values.
        good.replace("githop-export", "something-else"),
        good.replace("color = \"blue\"", "color = \"red\""),
        good.replace("enforce = \"block\"", "enforce = \"off\""),
        good.replace("log_retention_days = 7", "log_retention_days = 0"),
        good.replace("email = \"me@company.com\"", "email = \"not an email\""),
        good.replace("id = \"office\"", "id = \"Bad Id\""),
        good.replace("label = \"Office\"", "label = \"Off\\u001b[31mice\""),
        good.replace("private = \"", "private = \"!!!"),
        good.replace(
            "public = \"ssh-ed25519 AAAA test\"",
            "public = \"rm -rf /\"",
        ),
        good.replace("default = \"office\"", "default = \"nobody\""),
        // Key that no profile uses.
        good.replace("name = \"githop_office\"", "name = \"githop_other\""),
        String::new(),
        "format = \"githop-export\"\n".to_string(),
    ];
    for (i, text) in cases.iter().enumerate() {
        assert!(
            parse_bundle(text.as_bytes()).is_err(),
            "case {i} should fail:\n{text}"
        );
    }

    let newer = good.replace("version = 1\ncreated", "version = 2\ncreated");
    assert!(matches!(
        parse_bundle(newer.as_bytes()),
        Err(ExportError::UnsupportedVersion { found: 2, .. })
    ));
}

#[test]
fn path_traversal_key_names_are_rejected() {
    let hostile = [
        "../x",
        "..\\x",
        "C:\\x",
        "C:/Windows/x",
        "/etc/passwd",
        "\\\\server\\share\\x",
        "githop_a/../../b",
        "githop_a\\..\\b",
        "githop_..",
        "CON",
        "githop_con.txt",
        "NUL",
        "githop_\u{e9}t\u{e9}",
        "githop_\u{ff0e}\u{ff0e}",
        "githop_a b",
        "id_ed25519",
        "",
    ];
    for name in hostile {
        let escaped = name.replace('\\', "\\\\");
        // In the profile's ssh_key …
        let text = bundle_text(&escaped, "githop_office");
        assert!(
            matches!(
                parse_bundle(text.as_bytes()),
                Err(ExportError::UnsafeKeyName(_))
            ),
            "profile ssh_key {name:?} should be rejected"
        );
        // … and in the [[keys]] entry.
        let text = bundle_text("githop_office", &escaped);
        assert!(
            matches!(
                parse_bundle(text.as_bytes()),
                Err(ExportError::UnsafeKeyName(_))
            ),
            "key name {name:?} should be rejected"
        );
    }
}

/// Deterministic xorshift so the "fuzz" loop is reproducible.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn mutate(rng: &mut Rng, input: &[u8]) -> Vec<u8> {
    let mut v = input.to_vec();
    let edits = 1 + rng.below(4);
    for _ in 0..edits {
        match rng.below(6) {
            0 if !v.is_empty() => {
                let i = rng.below(v.len());
                v[i] ^= 1 << rng.below(8);
            }
            1 if !v.is_empty() => {
                let i = rng.below(v.len());
                v[i] = (rng.next() & 0xff) as u8;
            }
            2 if !v.is_empty() => {
                let at = rng.below(v.len());
                v.truncate(at);
            }
            3 => {
                let i = rng.below(v.len() + 1);
                let tokens: [&[u8]; 8] = [
                    b"\"",
                    b"[",
                    b"]]",
                    b"\n[[keys]]\n",
                    b"=",
                    b"\\u0000",
                    b"../",
                    b"\xff\xfe",
                ];
                let t = tokens[rng.below(tokens.len())];
                v.splice(i..i, t.iter().copied());
            }
            4 if v.len() > 2 => {
                let a = rng.below(v.len());
                let b = rng.below(v.len());
                let (lo, hi) = if a < b { (a, b) } else { (b, a) };
                let chunk = v[lo..hi].to_vec();
                v.splice(lo..lo, chunk);
            }
            _ => v.push((rng.next() & 0xff) as u8),
        }
    }
    v
}

#[test]
fn mutated_bundles_never_panic() {
    let good = bundle_text("githop_office", "githop_office").into_bytes();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut accepted = 0;
    for _ in 0..3000 {
        let input = mutate(&mut rng, &good);
        if let Ok(bundle) = parse_bundle(&input) {
            accepted += 1;
            // Anything accepted still only names safe keys.
            for name in bundle.key_names() {
                assert!(export::is_safe_key_name(&name), "{name:?}");
            }
        }
    }
    // Pure garbage of every size.
    for len in [0usize, 1, 7, 64, 4096, 70_000] {
        let junk: Vec<u8> = (0..len).map(|_| (rng.next() & 0xff) as u8).collect();
        let _ = parse_bundle(&junk);
        let _ = decrypt_bytes(&junk, PASSWORD);
    }
    assert!(accepted < 3000, "mutations should mostly be rejected");
}

#[test]
fn mutated_ciphertext_never_panics() {
    let plain = bundle_text("githop_office", "githop_office");
    let good = encrypt_bytes(plain.as_bytes(), PASSWORD, 1).unwrap();
    let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
    for _ in 0..400 {
        let input = mutate(&mut rng, &good);
        if let Ok(out) = decrypt_bytes(&input, PASSWORD) {
            // Only an unchanged payload may decrypt (age is authenticated).
            assert_eq!(out.as_slice(), plain.as_bytes());
        }
    }
    // An age file with a huge work factor is refused without doing the work.
    let text = String::from_utf8_lossy(&good).into_owned();
    let hostile = text.replacen(" 1\n", " 40\n", 1);
    assert!(matches!(
        decrypt_bytes(hostile.as_bytes(), PASSWORD),
        Err(ExportError::ExcessiveWork) | Err(ExportError::Corrupt)
    ));
}

// ---------------------------------------------------------------- conflicts

/// "Machine B" with its own Office profile (same id, same email) and a
/// different key file at `~/.ssh/githop_office`.
fn target_with_office() -> TestEnv {
    let b = TestEnv::new();
    write_key(&b, "githop_office", "b-office-original");
    let mut office = profile(
        &b,
        "office",
        OFFICE_EMAIL,
        ProfileColor::Violet,
        "office-org",
    );
    office.label = "Work".into();
    let other = profile(
        &b,
        "side",
        "side@example.com",
        ProfileColor::Blue,
        "side-org",
    );
    let set = ProfileSet {
        version: 1,
        default: Some("office".into()),
        profiles: vec![other, office],
    };
    store(&b).save(&set, &Settings::default()).unwrap();
    b
}

#[test]
fn conflicts_are_listed_and_must_be_resolved() {
    let a = source_machine();
    let file = export_file(&a, true);
    let b = target_with_office();
    let bundle = open_bundle(&file, PASSWORD).unwrap();

    let preview = preview_bundle(&b.paths(), &b.git_handle(), &bundle).unwrap();
    let office: Vec<ConflictKind> = preview
        .conflicts
        .iter()
        .filter(|c| c.profile_id == "office")
        .map(|c| c.kind)
        .collect();
    for kind in [
        ConflictKind::Id,
        ConflictKind::Email,
        ConflictKind::Color,
        ConflictKind::Owner,
        ConflictKind::KeyFile,
    ] {
        assert!(
            office.contains(&kind),
            "{kind:?} missing: {:?}",
            preview.conflicts
        );
    }
    let key_conflict = preview
        .conflicts
        .iter()
        .find(|c| c.kind == ConflictKind::KeyFile)
        .unwrap();
    assert_eq!(key_conflict.existing_id, "office");
    // Personal only clashes on colour (fixed automatically).
    assert!(preview
        .conflicts
        .iter()
        .filter(|c| c.profile_id == "personal")
        .all(|c| c.kind == ConflictKind::Color));
    assert!(!preview.include_settings);

    let r = apply_bundle(&b.paths(), &b.git_handle(), &bundle, &[]);
    assert!(
        matches!(&r, Err(ExportError::Unresolved(ids)) if ids == &["office".to_string()]),
        "{r:?}"
    );
    // Nothing changed.
    assert_eq!(store(&b).profiles().profiles.len(), 2);

    let r = apply_bundle(
        &b.paths(),
        &b.git_handle(),
        &bundle,
        &[resolve("nobody", ImportAction::Skip, None)],
    );
    assert!(matches!(r, Err(ExportError::Conflict(_))), "{r:?}");
}

#[test]
fn skip_keeps_existing_profile_and_key() {
    let a = source_machine();
    let file = export_file(&a, true);
    let b = target_with_office();
    let before = std::fs::read(ssh_dir(&b).join("githop_office")).unwrap();
    let bundle = open_bundle(&file, PASSWORD).unwrap();

    let report = apply_bundle(
        &b.paths(),
        &b.git_handle(),
        &bundle,
        &[resolve("office", ImportAction::Skip, None)],
    )
    .unwrap();
    assert_eq!(report.skipped, ["office"]);
    assert_eq!(report.imported, ["personal"]);
    let s = store(&b);
    let ids: Vec<&str> = s
        .profiles()
        .profiles
        .iter()
        .map(|p| p.id.as_str())
        .collect();
    assert_eq!(ids, ["side", "office", "personal"]);
    assert_eq!(s.profiles().get("office").unwrap().label, "Work");
    // Personal's violet was taken by B's office: it got a free colour.
    let personal = s.profiles().get("personal").unwrap();
    assert_ne!(personal.color, ProfileColor::Violet);
    assert!(
        report.notes.iter().any(|n| n.contains("colour")),
        "{:?}",
        report.notes
    );
    assert_eq!(s.profiles().default.as_deref(), Some("office"));
    assert!(!report.default_changed);
    assert_eq!(
        std::fs::read(ssh_dir(&b).join("githop_office")).unwrap(),
        before
    );
}

#[test]
fn replace_swaps_profile_in_place_and_moves_old_key_aside() {
    let a = source_machine();
    let file = export_file(&a, true);
    let b = target_with_office();
    let before = std::fs::read(ssh_dir(&b).join("githop_office")).unwrap();
    let bundle = open_bundle(&file, PASSWORD).unwrap();

    let report = apply_bundle(
        &b.paths(),
        &b.git_handle(),
        &bundle,
        &[
            resolve("office", ImportAction::Replace, None),
            resolve("personal", ImportAction::Skip, None),
        ],
    )
    .unwrap();
    assert_eq!(report.replaced, ["office"]);
    let s = store(&b);
    let ids: Vec<&str> = s
        .profiles()
        .profiles
        .iter()
        .map(|p| p.id.as_str())
        .collect();
    assert_eq!(ids, ["side", "office"]);
    let office = s.profiles().get("office").unwrap();
    assert_eq!(office.label, "Office");
    assert_eq!(s.profiles().default.as_deref(), Some("office"));

    // New key in place, old one kept next to it under a new name.
    let new_key = std::fs::read(ssh_dir(&b).join("githop_office")).unwrap();
    assert_eq!(
        new_key,
        std::fs::read(ssh_dir(&a).join("githop_office")).unwrap()
    );
    assert_eq!(
        report.keys_moved_aside.len(),
        2,
        "{:?}",
        report.keys_moved_aside
    );
    let moved = &report.keys_moved_aside[0];
    assert!(moved.to_string_lossy().contains(".replaced-"));
    assert_eq!(std::fs::read(moved).unwrap(), before);
    assert_user_only(&ssh_dir(&b).join("githop_office"));
}

#[test]
fn rename_imports_under_new_id_with_its_own_key() {
    let a = source_machine();
    let file = export_file(&a, true);
    let b = TestEnv::new();
    // Same id, different email, existing different key file.
    write_key(&b, "githop_office", "b-office");
    let office = profile(
        &b,
        "office",
        "other@company.com",
        ProfileColor::Blue,
        "office-org",
    );
    let set = ProfileSet {
        version: 1,
        default: Some("office".into()),
        profiles: vec![office],
    };
    store(&b).save(&set, &Settings::default()).unwrap();
    let before = std::fs::read(ssh_dir(&b).join("githop_office")).unwrap();
    let bundle = open_bundle(&file, PASSWORD).unwrap();

    // A taken new id is refused.
    let r = apply_bundle(
        &b.paths(),
        &b.git_handle(),
        &bundle,
        &[resolve("office", ImportAction::Rename, Some("office"))],
    );
    assert!(matches!(r, Err(ExportError::Conflict(_))), "{r:?}");

    let report = apply_bundle(
        &b.paths(),
        &b.git_handle(),
        &bundle,
        &[resolve("office", ImportAction::Rename, None)],
    )
    .unwrap();
    assert_eq!(
        report.renamed,
        [("office".to_string(), "office-2".to_string())]
    );
    let s = store(&b);
    let renamed = s.profiles().get("office-2").unwrap();
    assert_eq!(renamed.email, OFFICE_EMAIL);
    // The owner already belongs to B's office, so it was dropped from the copy.
    assert!(renamed.owners.is_empty());
    assert!(report.notes.iter().any(|n| n.contains("owners")));
    assert_eq!(
        slash(&renamed.ssh_key),
        slash(&ssh_dir(&b).join("githop_office-2"))
    );
    assert_eq!(
        std::fs::read(ssh_dir(&b).join("githop_office-2")).unwrap(),
        std::fs::read(ssh_dir(&a).join("githop_office")).unwrap()
    );
    // B's own key untouched.
    assert_eq!(
        std::fs::read(ssh_dir(&b).join("githop_office")).unwrap(),
        before
    );
    assert_eq!(s.profiles().default.as_deref(), Some("office"));
}

#[test]
fn rename_cannot_fix_an_email_clash() {
    let a = source_machine();
    let file = export_file(&a, false);
    let b = target_with_office();
    let bundle = open_bundle(&file, PASSWORD).unwrap();
    let r = apply_bundle(
        &b.paths(),
        &b.git_handle(),
        &bundle,
        &[resolve("office", ImportAction::Rename, Some("office-copy"))],
    );
    assert!(
        matches!(&r, Err(ExportError::Conflict(m)) if m.contains("same email")),
        "{r:?}"
    );
}

#[test]
fn existing_key_file_is_never_overwritten_without_replace() {
    let a = source_machine();
    let file = export_file(&a, true);
    let b = TestEnv::new();
    // No profiles on B, but a foreign key file sits where Office's would go.
    write_key(&b, "githop_office", "foreign");
    let before = std::fs::read(ssh_dir(&b).join("githop_office")).unwrap();
    let bundle = open_bundle(&file, PASSWORD).unwrap();

    let preview = preview_bundle(&b.paths(), &b.git_handle(), &bundle).unwrap();
    let c: Vec<_> = preview
        .conflicts
        .iter()
        .filter(|c| c.kind == ConflictKind::KeyFile)
        .collect();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].profile_id, "office");
    assert_eq!(c[0].existing_id, "");

    assert!(matches!(
        apply_bundle(&b.paths(), &b.git_handle(), &bundle, &[]),
        Err(ExportError::Unresolved(_))
    ));
    assert_eq!(
        std::fs::read(ssh_dir(&b).join("githop_office")).unwrap(),
        before
    );
}
