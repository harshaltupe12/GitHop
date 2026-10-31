mod support;

use support::TestEnv;

/// Proves the isolated environment works: commit identity comes from the temp
/// global config, never from the developer's real one.
#[test]
fn isolated_commit_uses_test_identity() {
    let env = TestEnv::new();
    env.set_global("user.name", "Test Person");
    env.set_global("user.email", "test@example.invalid");
    let repo = env.init_repo("smoke");
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    env.git(&["add", "a.txt"], &repo);
    env.git(&["commit", "-q", "-m", "first"], &repo);
    assert_eq!(
        env.git(&["log", "-1", "--format=%ae"], &repo),
        "test@example.invalid"
    );
}

/// Best-effort guard: the real global config is not modified by the test env.
#[test]
fn real_global_config_untouched() {
    let real = dirs::home_dir().map(|h| h.join(".gitconfig"));
    let before = real
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());
    let env = TestEnv::new();
    env.set_global("user.email", "isolated@example.invalid");
    let after = real
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());
    assert_eq!(before, after);
}
