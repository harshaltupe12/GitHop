//! `githop list [--json]` — profiles and which one is the default profile.

use anyhow::Result;
use githop_core::ProfileSet;
use serde_json::json;

use super::{key_display, Ctx};
use crate::exit::Exit;

pub fn run(ctx: &Ctx) -> Result<Exit> {
    let store = ctx.store()?;
    let set = store.profiles();
    if ctx.ui.json {
        ctx.ui.json_out(&list_json(set))?;
        return Ok(Exit::Ok);
    }
    if set.profiles.is_empty() {
        ctx.ui.line("No profiles yet.");
        ctx.ui.hint("Run: githop init");
        return Ok(Exit::Ok);
    }
    // Colour names only with -v: emails must fit the 80-column limit untruncated.
    let dot = if ctx.ui.ascii { "*" } else { "●" };
    let rows: Vec<Vec<String>> = set
        .profiles
        .iter()
        .map(|p| {
            vec![
                if ctx.ui.verbose {
                    format!("{dot} {}", p.color.as_str())
                } else {
                    dot.to_string()
                },
                p.id.clone(),
                p.label.clone(),
                p.email.clone(),
                if p.owners.is_empty() {
                    "—".to_string()
                } else {
                    p.owners.join(", ")
                },
                if set.default.as_deref() == Some(p.id.as_str()) {
                    "default".to_string()
                } else {
                    String::new()
                },
            ]
        })
        .collect();
    ctx.ui
        .table(&["", "Id", "Label", "Email", "Owners", ""], &rows);
    if set.default_profile().is_none() {
        ctx.ui.hint("No default profile yet. Run: githop use <id>");
    }
    Ok(Exit::Ok)
}

/// Schema: docs/cli-json.md#githop-list.
pub fn list_json(set: &ProfileSet) -> serde_json::Value {
    let profiles: Vec<serde_json::Value> = set
        .profiles
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "label": p.label,
                "name": p.name,
                "email": p.email,
                "color": p.color.as_str(),
                "ssh_key": key_display(&p.ssh_key),
                "owners": p.owners,
                "folders": p.folders,
                "default": set.default.as_deref() == Some(p.id.as_str()),
            })
        })
        .collect();
    json!({ "default": set.default, "profiles": profiles })
}

#[cfg(test)]
mod tests {
    use super::*;
    use githop_core::{Profile, ProfileColor};

    #[test]
    fn json_shape() {
        let set = ProfileSet {
            version: 1,
            default: Some("office".into()),
            profiles: vec![Profile {
                id: "office".into(),
                label: "Office".into(),
                name: "Me".into(),
                email: "me@company.com".into(),
                color: ProfileColor::Blue,
                ssh_key: "C:/k".into(),
                owners: vec!["office-org".into()],
                folders: vec![],
                sign_commits: false,
            }],
        };
        let v = list_json(&set);
        assert_eq!(v["default"], "office");
        assert_eq!(v["profiles"][0]["color"], "blue");
        assert_eq!(v["profiles"][0]["default"], true);
        assert_eq!(v["profiles"][0]["owners"][0], "office-org");
    }
}
