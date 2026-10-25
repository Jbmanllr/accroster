//! What a Profile folder is made of before and after its sign-in: a fixed id
//! and path, links back to `~/.claude`, a one-time `.claude.json` seed, and a
//! clean environment for every Claude Code spawn in it.

mod common;

use std::fs;
use std::path::Path;

use accroster_fake_mac::{FakeMac, FixtureLogin, Reply, StubCall};
use common::*;
use serde_json::json;

const SHARED_DIRS: [&str; 11] = [
    "projects",
    "file-history",
    "tasks",
    "plans",
    "session-env",
    "skills",
    "agents",
    "commands",
    "hooks",
    "output-styles",
    "plugins",
];

fn link_target(mac: &FakeMac, relative: &str) -> Option<std::path::PathBuf> {
    fs::read_link(mac.path(relative)).ok()
}

#[test]
fn the_folder_path_is_stored_once_and_every_spawn_uses_it_verbatim() {
    let mac = mac_signing_in_as(&work(), limits_body());
    let mut core = mac.launch();

    add_profile(&mut core);

    let folder = only_profile_folder(&mac);
    let id = folder.rsplit('/').next().unwrap().to_owned();
    let record = mac
        .read_json(format!("{APP_FOLDER}/state/profiles.json"))
        .expect("the Profile is recorded");
    let stored = mac.path(&folder).to_string_lossy().into_owned();
    assert_eq!(
        record,
        json!({ "version": 1, "profiles": [ { "id": id, "configDir": stored } ] })
    );
    let calls = mac.claude.calls();
    assert_eq!(calls.len(), 3);
    for call in &calls {
        assert_eq!(
            call.var("CLAUDE_CONFIG_DIR"),
            Some(stored.as_str()),
            "{:?}",
            call.kind
        );
    }
}

#[test]
fn before_sign_in_the_folder_links_back_to_the_shared_claude_folder() {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    mac.write_file(".claude/CLAUDE.md", "# mine\n");
    mac.claude.reply(StubCall::AuthLogin, Reply::hanging());
    let mut core = mac.launch();

    click(&mut core, "Add Profile…");
    sign_in_call(&mac);

    let folder = only_profile_folder(&mac);
    for dir in SHARED_DIRS {
        assert_eq!(
            link_target(&mac, &format!("{folder}/{dir}")),
            Some(mac.path(format!(".claude/{dir}"))),
            "{dir}"
        );
        assert!(
            mac.path(format!(".claude/{dir}")).is_dir(),
            "{dir} made in ~/.claude"
        );
    }
    assert_eq!(
        link_target(&mac, &format!("{folder}/settings.json")),
        Some(mac.path(".claude/settings.json"))
    );
    assert_eq!(
        link_target(&mac, &format!("{folder}/CLAUDE.md")),
        Some(mac.path(".claude/CLAUDE.md"))
    );
    assert!(
        fs::symlink_metadata(mac.path(format!("{folder}/keybindings.json"))).is_err(),
        "no link to a file ~/.claude doesn't have"
    );
    for kept in ["history.jsonl", "sessions", "backups", "cache"] {
        assert!(
            fs::symlink_metadata(mac.path(format!("{folder}/{kept}"))).is_err(),
            "{kept} stays per Profile"
        );
    }
    click(&mut core, WAITING);
}

#[test]
fn before_sign_in_claude_json_is_seeded_with_shared_choices_but_no_identity() {
    let mac = FakeMac::new();
    mac.with_claude_home(&FixtureLogin::perso());
    mac.write_json(
        ".claude.json",
        &json!({
            "oauthAccount": FixtureLogin::perso().oauth_account(),
            "userID": "made-up-user-id",
            "anonymousId": "made-up-anonymous-id",
            "cachedUsageUtilization": { "fetchedAtMs": 1, "accountUuid": "x", "utilization": {} },
            "modelAccessCache": { "x": true },
            "cachedGrowthBookFeatures": { "x": 1 },
            "mcpServers": { "docs": { "type": "stdio", "command": "docs-mcp" } },
            "theme": "dark",
            "hasCompletedOnboarding": true,
            "numStartups": 12,
            "projects": {
                "/Users/tester/code/app": {
                    "hasTrustDialogAccepted": true,
                    "allowedTools": ["Bash(ls)"],
                    "enabledMcpjsonServers": ["db"],
                    "disabledMcpjsonServers": [],
                    "disabledMcpServers": ["slack"],
                    "mcpServers": { "local": { "type": "stdio", "command": "local-mcp" } },
                    "hasClaudeMdExternalIncludesApproved": true,
                    "lastSessionFirstPrompt": "a private prompt",
                    "lastCost": 1.5,
                    "lastSessionId": "made-up-session",
                },
                "/Users/tester/code/untouched": { "lastCost": 0.2 },
            },
        }),
    );
    mac.claude.reply(StubCall::AuthLogin, Reply::hanging());
    let mut core = mac.launch();

    click(&mut core, "Add Profile…");
    sign_in_call(&mac);

    let folder = only_profile_folder(&mac);
    assert_eq!(
        mac.read_json(format!("{folder}/.claude.json")),
        Some(json!({
            "mcpServers": { "docs": { "type": "stdio", "command": "docs-mcp" } },
            "theme": "dark",
            "hasCompletedOnboarding": true,
            "projects": {
                "/Users/tester/code/app": {
                    "hasTrustDialogAccepted": true,
                    "allowedTools": ["Bash(ls)"],
                    "enabledMcpjsonServers": ["db"],
                    "disabledMcpjsonServers": [],
                    "disabledMcpServers": ["slack"],
                    "mcpServers": { "local": { "type": "stdio", "command": "local-mcp" } },
                    "hasClaudeMdExternalIncludesApproved": true,
                },
            },
        }))
    );
    click(&mut core, WAITING);
}

#[test]
fn launch_remakes_a_missing_link_but_never_overwrites_a_file_in_its_place() {
    let mac = mac_signing_in_as(&work(), limits_body());
    let mut core = mac.launch();
    add_profile(&mut core);
    drop(core);
    let folder = only_profile_folder(&mac);
    fs::remove_file(mac.path(format!("{folder}/plans"))).unwrap();
    fs::remove_file(mac.path(format!("{folder}/settings.json"))).unwrap();
    mac.write_file(format!("{folder}/settings.json"), "{\"mine\": true}\n");

    let _core = mac.launch();

    assert_eq!(
        link_target(&mac, &format!("{folder}/plans")),
        Some(mac.path(".claude/plans"))
    );
    assert_eq!(
        mac.read_file(format!("{folder}/settings.json")).as_deref(),
        Some("{\"mine\": true}\n"),
        "the regular file is left as it was"
    );
    assert!(link_target(&mac, &format!("{folder}/settings.json")).is_none());
}

#[test]
fn launch_deletes_what_an_interrupted_add_profile_left_behind() {
    let mac = mac_signing_in_as(&work(), limits_body());
    let mut core = mac.launch();
    add_profile(&mut core);
    drop(core);
    let kept = only_profile_folder(&mac);
    // A folder from a sign-in the app quit during: seeded, never signed in.
    mac.write_json(format!("{PROFILES}/0123456789ab/.claude.json"), &json!({}));
    std::os::unix::fs::symlink(
        mac.path(".claude/projects"),
        mac.path(format!("{PROFILES}/0123456789ab/projects")),
    )
    .unwrap();

    let core = mac.launch();

    assert_eq!(mac.list(PROFILES).len(), 1);
    assert!(mac.path(&kept).is_dir());
    assert!(
        mac.path(".claude/projects").is_dir(),
        "links were not followed"
    );
    assert_eq!(rows(&core).len(), 1);
}

#[test]
fn spawns_clear_inherited_claude_and_anthropic_variables_and_never_use_dot_claude() {
    let mac = mac_signing_in_as(&work(), limits_body());
    let dot_claude = mac.path(".claude").to_string_lossy().into_owned();
    mac.inherit_var("CLAUDE_CONFIG_DIR", &dot_claude);
    mac.inherit_var(
        "CLAUDE_CODE_OAUTH_TOKEN",
        "sk-ant-oat01-injected-by-the-desktop",
    );
    mac.inherit_var(
        "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
        "sk-ant-ort01-someone-elses",
    );
    mac.inherit_var("CLAUDE_SECURESTORAGE_CONFIG_DIR", "");
    mac.inherit_var("CLAUDECODE", "1");
    mac.inherit_var("ANTHROPIC_API_KEY", "sk-ant-api03-deliberate");
    mac.inherit_var("ANTHROPIC_BASE_URL", "https://proxy.example");
    mac.inherit_var("LANG", "en_US.UTF-8");
    let mut core = mac.launch();

    add_profile(&mut core);

    let folder = mac.path(only_profile_folder(&mac));
    let calls = mac.claude.calls();
    assert_eq!(calls.len(), 3);
    for call in &calls {
        let claude_vars: Vec<(&String, &String)> = call
            .env
            .iter()
            .filter(|(name, _)| name.starts_with("CLAUDE") || name.starts_with("ANTHROPIC_"))
            .collect();
        let config_dir = folder.to_string_lossy().into_owned();
        assert_eq!(
            claude_vars,
            [(&"CLAUDE_CONFIG_DIR".to_string(), &config_dir)],
            "{:?}",
            call.kind
        );
        assert_ne!(call.var("CLAUDE_CONFIG_DIR"), Some(dot_claude.as_str()));
        assert_eq!(
            call.var("LANG"),
            Some("en_US.UTF-8"),
            "other variables pass"
        );
        assert_eq!(call.var("HOME"), Some(&*mac.home().to_string_lossy()));
        // A neutral working directory, outside every project.
        let cwd = call.var("PWD").expect("sh sets PWD");
        assert!(!Path::new(cwd).starts_with(mac.path(".claude")), "{cwd}");
    }
}
