//! Command-line definitions (clap derive). Help text: sentence case, glossary words
//! (Profile, Default profile, Owner, Pin, Blocked, Fix, Wipe) — Design §9.2.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Give each git identity its own name, email and SSH key, and block commits
/// made with the wrong one.
#[derive(Debug, Parser)]
#[command(
    name = "githop",
    bin_name = "githop",
    version,
    about = "Hop between git identities safely",
    long_about = "Give each git identity (Office, Personal, …) its own name, email and SSH key.\n\
                  GitHop picks the right profile for each repo and blocks commits and pushes\n\
                  made with the wrong one.",
    after_help = "Exit codes: 0 ok · 1 blocked/problem found · 2 usage error · 3 internal error",
    propagate_version = true
)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,

    #[command(subcommand)]
    pub command: Command,
}

/// Flags accepted by every command.
#[derive(Debug, Clone, Default, Args)]
pub struct GlobalArgs {
    /// JSON output where available (list, status, fix, …)
    #[arg(long, global = true)]
    pub json: bool,

    /// Turn off colours (NO_COLOR is also respected)
    #[arg(long = "no-color", global = true)]
    pub no_color: bool,

    /// Only print warnings and problems
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Print extra detail
    #[arg(short, long, global = true)]
    pub verbose: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Set up GitHop: back up git config, create a profile, install hooks
    Init(InitArgs),

    /// Add a profile
    Add(AddArgs),

    /// Change a profile
    Edit(EditArgs),

    /// Remove a profile (its SSH key is kept)
    Remove(RemoveArgs),

    /// List profiles and show which one is the default profile
    List,

    /// Make a profile the default profile (global name, email and key)
    Use(UseArgs),

    /// Show which profile this repo uses, and why
    Status,

    /// Pin this repo to a profile
    Pin(PinArgs),

    /// Remove this repo's pin
    Unpin,

    /// Find repos in a folder and show which profile owns each
    Scan(ScanArgs),

    /// Create an SSH key for a profile and print its public key
    Keygen(KeygenArgs),

    /// Test a profile's SSH key against GitHub (connects to github.com)
    Test(TestArgs),

    /// Check your setup and suggest fixes
    Doctor,

    /// Save your profiles to a password-protected file for another PC
    Export(crate::cmd::export::ExportArgs),

    /// Add profiles from a file made by githop export
    Import(crate::cmd::import::ImportArgs),

    /// Wipe GitHop: restore original git settings, delete GitHop data
    Wipe(WipeArgs),

    /// Check the identity of a clone, commit or push (used by hooks)
    Check(CheckArgs),

    /// Install, remove or check GitHop's git hooks
    #[command(subcommand)]
    Hooks(HooksCommand),

    /// Fix unpushed commits that used the wrong identity
    Fix(FixArgs),
}

#[derive(Debug, Clone, Default, Args)]
pub struct InitArgs {
    /// Profile id for your current identity, e.g. "office"
    #[arg(long)]
    pub id: Option<String>,
    /// Display name, e.g. "Office"
    #[arg(long)]
    pub label: Option<String>,
    /// git user.name (defaults to your global user.name)
    #[arg(long)]
    pub name: Option<String>,
    /// git user.email (defaults to your global user.email)
    #[arg(long)]
    pub email: Option<String>,
    /// Profile colour
    #[arg(long, value_parser = parse_color_name)]
    pub color: Option<String>,
    /// Path to the profile's private SSH key
    #[arg(long)]
    pub key: Option<PathBuf>,
    /// Don't ask for confirmation
    #[arg(short, long)]
    pub yes: bool,
    /// Skip the hooks (commits and pushes aren't checked)
    #[arg(long)]
    pub no_hooks: bool,
}

#[derive(Debug, Clone, Default, Args)]
pub struct AddArgs {
    /// Profile id (a-z, 0-9, dashes), e.g. "personal"
    #[arg(long)]
    pub id: Option<String>,
    /// Display name, e.g. "Personal" (defaults from the id)
    #[arg(long)]
    pub label: Option<String>,
    /// git user.name
    #[arg(long)]
    pub name: Option<String>,
    /// git user.email
    #[arg(long)]
    pub email: Option<String>,
    /// Profile colour (defaults to the first free one)
    #[arg(long, value_parser = parse_color_name)]
    pub color: Option<String>,
    /// GitHub user/org whose repos use it (repeatable)
    #[arg(long = "owner", value_name = "OWNER")]
    pub owners: Vec<String>,
    /// Folder whose repos use this profile (repeatable)
    #[arg(long = "folder", value_name = "FOLDER")]
    pub folders: Vec<String>,
    /// Private SSH key path (default: ~/.ssh/githop_<id>)
    #[arg(long)]
    pub key: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Args)]
pub struct EditArgs {
    /// Profile to change
    pub id: String,
    /// New display name
    #[arg(long)]
    pub label: Option<String>,
    /// New git user.name
    #[arg(long)]
    pub name: Option<String>,
    /// New git user.email
    #[arg(long)]
    pub email: Option<String>,
    /// New colour
    #[arg(long, value_parser = parse_color_name)]
    pub color: Option<String>,
    /// Replace the owners; repeat for more
    #[arg(long = "owner", value_name = "OWNER", conflicts_with = "no_owners")]
    pub owners: Vec<String>,
    /// Remove all owners
    #[arg(long)]
    pub no_owners: bool,
    /// Replace the folders; repeat for more
    #[arg(long = "folder", value_name = "FOLDER", conflicts_with = "no_folders")]
    pub folders: Vec<String>,
    /// Remove all folders
    #[arg(long)]
    pub no_folders: bool,
    /// New path to the private SSH key
    #[arg(long)]
    pub key: Option<PathBuf>,
    /// Re-pin repos pinned to this profile without asking
    #[arg(long)]
    pub repin: bool,
}

#[derive(Debug, Clone, Default, Args)]
pub struct RemoveArgs {
    /// Profile to remove
    pub id: String,
    /// Don't ask for confirmation
    #[arg(short, long)]
    pub yes: bool,
    /// New default profile, when removing the current one
    #[arg(long, value_name = "ID")]
    pub new_default: Option<String>,
}

#[derive(Debug, Clone, Default, Args)]
pub struct UseArgs {
    /// Profile to make the default profile
    pub id: String,
}

#[derive(Debug, Clone, Default, Args)]
pub struct PinArgs {
    /// Profile to pin this repo to
    pub id: String,
}

#[derive(Debug, Clone, Default, Args)]
pub struct ScanArgs {
    /// Folder to search for repos
    pub folder: PathBuf,
    /// Pin unpinned repos matched by an owner or folder rule
    #[arg(long)]
    pub apply: bool,
    /// With --apply, also pin repos matched only by default
    #[arg(long, requires = "apply")]
    pub include_default: bool,
    /// Folder levels to search
    #[arg(long, default_value_t = githop_core::scan::DEFAULT_MAX_DEPTH)]
    pub depth: usize,
}

#[derive(Debug, Clone, Default, Args)]
pub struct KeygenArgs {
    /// Profile to create a key for
    pub id: String,
    /// Create the key without a passphrase (not recommended)
    #[arg(long)]
    pub no_passphrase: bool,
}

#[derive(Debug, Clone, Default, Args)]
pub struct TestArgs {
    /// Profile whose key to test
    pub id: String,
}

#[derive(Debug, Clone, Default, Args)]
pub struct WipeArgs {
    /// Don't ask (also unpins repos GitHop pinned)
    #[arg(short, long)]
    pub yes: bool,
    /// Keep SSH keys (the default)
    #[arg(long, conflicts_with = "delete_keys")]
    pub keep_keys: bool,
    /// Also delete SSH keys GitHop created (~/.ssh/githop_*)
    #[arg(long)]
    pub delete_keys: bool,
    /// Keep the pins in repos GitHop pinned
    #[arg(long)]
    pub keep_pins: bool,
    /// Keep GitHop's data folder (profiles, backups)
    #[arg(long)]
    pub keep_data: bool,
    /// Run from the uninstaller: never ask, keep SSH keys
    #[arg(long, conflicts_with = "delete_keys")]
    pub from_uninstaller: bool,
}

#[derive(Debug, Subcommand)]
pub enum HooksCommand {
    /// Install or repair the hooks (sets global core.hooksPath)
    Install(HooksInstallArgs),
    /// Remove the hooks and restore your previous core.hooksPath
    Uninstall,
    /// Show whether the hooks are installed and healthy
    Status,
    /// Print a line to call GitHop from a repo's own hook (husky etc.)
    Snippet(HooksSnippetArgs),
}

#[derive(Debug, Clone, Default, Args)]
pub struct HooksInstallArgs {
    /// Don't ask for confirmation
    #[arg(short, long)]
    pub yes: bool,
    /// Also pass through reference-transaction and post-index-change (slower)
    #[arg(long, conflicts_with = "common_stubs")]
    pub all_stubs: bool,
    /// Skip those two rarely used hooks again (the default)
    #[arg(long)]
    pub common_stubs: bool,
}

#[derive(Debug, Clone, Args)]
pub struct HooksSnippetArgs {
    /// Hook stage the line is for
    #[arg(value_enum)]
    pub stage: Stage,
}

/// Hook stage for `githop check`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Stage {
    Clone,
    Commit,
    Push,
}

#[derive(Debug, Clone, Args)]
pub struct CheckArgs {
    /// Hook stage
    #[arg(long, value_enum)]
    pub stage: Stage,
    /// Arguments git passed to the hook
    #[arg(last = true)]
    pub hook_args: Vec<String>,
}

/// `githop fix`: only unpushed commits are changed, and a backup is saved first.
#[derive(Debug, Clone, Default, Args)]
#[command(
    after_help = "Only unpushed commits are changed. A backup is saved first, so you can undo."
)]
pub struct FixArgs {
    /// Don't ask before fixing or undoing
    #[arg(short, long)]
    pub yes: bool,
    /// Show which commits would be fixed, change nothing
    #[arg(long, conflicts_with = "undo")]
    pub dry_run: bool,
    /// Undo a fix: restore the newest (or given) backup
    #[arg(
        long,
        value_name = "BACKUP_REF",
        num_args = 0..=1,
        default_missing_value = ""
    )]
    pub undo: Option<String>,
}

/// Accept only known profile colour names (case-insensitive).
fn parse_color_name(s: &str) -> Result<String, String> {
    match githop_core::ProfileColor::parse(s) {
        Some(c) => Ok(c.as_str().to_string()),
        None => {
            let names: Vec<&str> = githop_core::ProfileColor::ALL
                .iter()
                .map(|c| c.as_str())
                .collect();
            Err(format!("pick one of: {}", names.join(", ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn clap_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn color_parser() {
        assert_eq!(parse_color_name("Blue"), Ok("blue".to_string()));
        assert!(parse_color_name("green").is_err());
    }

    #[test]
    fn check_takes_trailing_args() {
        let cli = Cli::try_parse_from(["githop", "check", "--stage", "push", "--", "origin", "x"])
            .unwrap();
        match cli.command {
            Command::Check(a) => {
                assert_eq!(a.stage, Stage::Push);
                assert_eq!(a.hook_args, vec!["origin", "x"]);
            }
            _ => panic!("wrong command"),
        }
    }
}
