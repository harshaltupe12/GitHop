use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn version_prints_crate_version() {
    Command::cargo_bin("githop")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains(env!("CARGO_PKG_VERSION")));
}
