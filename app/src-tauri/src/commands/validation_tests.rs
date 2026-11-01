//! Commands reject bad input before touching data, git or the file system.
//! These call the command functions directly (no WebView); every case fails in
//! validation, so nothing reads `GITHOP_HOME` or spawns git.

use tauri::async_runtime::block_on;

use githop_core::export::{ImportAction, ImportResolution};

use crate::dto::ErrorCode;

use super::{fix, keys, profiles, repos, settings};

const BAD_IDS: [&str; 6] = [
    "",
    "Office",
    "../x",
    "a/b",
    "x\n",
    "toolong-toolong-toolong-toolong-x",
];
const BAD_PATHS: [&str; 5] = ["", "relative/dir", "C:relative", "C:/a/../b", r"\\?\C:\x"];

fn code<T: std::fmt::Debug>(r: Result<T, crate::dto::IpcError>) -> ErrorCode {
    r.expect_err("command must reject this input").code
}

#[test]
fn id_commands_reject_bad_ids() {
    for id in BAD_IDS {
        assert_eq!(
            code(block_on(keys::suggest_key_path(id.into()))),
            ErrorCode::BadInput
        );
        assert_eq!(
            code(block_on(keys::read_public_key(id.into()))),
            ErrorCode::BadInput
        );
        assert_eq!(
            code(block_on(keys::test_connection(id.into()))),
            ErrorCode::BadInput
        );
    }
}

#[test]
fn validate_profile_rejects_bad_original_id() {
    let p = githop_core::Profile {
        id: "office".into(),
        label: "Office".into(),
        name: "Me".into(),
        email: "me@company.com".into(),
        color: githop_core::ProfileColor::Blue,
        ssh_key: "C:/k".into(),
        owners: vec![],
        folders: vec![],
    };
    for id in BAD_IDS {
        assert_eq!(
            code(block_on(profiles::validate_profile(
                p.clone(),
                Some(id.into())
            ))),
            ErrorCode::BadInput
        );
    }
}

#[test]
fn path_commands_reject_relative_and_traversal() {
    for path in BAD_PATHS {
        assert_eq!(
            code(block_on(fix::fix_plan(path.into()))),
            ErrorCode::BadInput
        );
        assert_eq!(
            code(block_on(settings::export_profiles(
                path.into(),
                "a long enough password".into(),
                false
            ))),
            ErrorCode::BadInput
        );
        assert_eq!(
            code(block_on(settings::import_preview(
                path.into(),
                "a long enough password".into()
            ))),
            ErrorCode::BadInput
        );
    }
}

#[test]
fn missing_folders_are_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let gone = tmp.path().join("gone").to_string_lossy().to_string();
    assert_eq!(
        code(block_on(fix::fix_plan(gone.clone()))),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(block_on(settings::import_preview(
            gone,
            "a long enough password".into()
        ))),
        ErrorCode::NotFound
    );
}

#[test]
fn passphrase_commands_reject_bad_ids() {
    for id in BAD_IDS {
        assert_eq!(
            code(block_on(keys::passphrase_state(id.into()))),
            ErrorCode::BadInput
        );
        assert_eq!(
            code(block_on(keys::remember_passphrase(id.into()))),
            ErrorCode::BadInput
        );
        assert_eq!(
            code(block_on(keys::turn_on_agent(id.into()))),
            ErrorCode::BadInput
        );
    }
}

#[test]
fn remote_names_are_validated() {
    for bad in ["", "-oProxyCommand=calc", "a b", "x\n", "a..b"] {
        assert!(crate::validate::remote_name(bad).is_err(), "{bad:?}");
    }
    for good in ["origin", "my-fork", "upstream_2", "team/main"] {
        assert!(crate::validate::remote_name(good).is_ok(), "{good:?}");
    }
}

#[test]
fn unknown_stage_is_rejected() {
    for stage in ["", "merge", "pre-commit", "COMMIT", "commit && calc"] {
        assert_eq!(
            code(block_on(repos::hooks_snippet(stage.into()))),
            ErrorCode::BadInput
        );
    }
}

#[test]
fn short_passwords_are_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("p.githop");
    std::fs::write(&file, "x").unwrap();
    let file = file.to_string_lossy().to_string();
    assert_eq!(
        code(block_on(settings::import_preview(
            file.clone(),
            "short".into()
        ))),
        ErrorCode::BadInput
    );
    let out = tmp.path().join("out.githop").to_string_lossy().to_string();
    assert_eq!(
        code(block_on(settings::export_profiles(
            out,
            "short".into(),
            true
        ))),
        ErrorCode::BadInput
    );
}

#[test]
fn bad_backup_ref_is_rejected_before_anything_runs() {
    // fix_undo needs an AppHandle, so check its validator directly with the
    // same inputs it would receive.
    for r in [
        "main",
        "refs/githop/backup/../x",
        "--upload-pack=x",
        "refs/githop/backup/a b",
    ] {
        assert!(crate::validate::backup_ref(r).is_err(), "{r}");
    }
    let bad = [ImportResolution {
        profile_id: "ok".into(),
        action: ImportAction::Rename,
        new_id: Some("Not Valid".into()),
    }];
    assert!(crate::validate::import_resolutions(&bad).is_err());
}
