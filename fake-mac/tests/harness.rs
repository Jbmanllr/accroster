//! The harness itself: the stub `claude`, reached the way the core reaches
//! it (the environment's spawner and `PATH`), answers from canned replies and
//! records what it was given. Later tickets rely on this behaviour.

use std::ffi::OsString;

use accroster_core::SpawnRequest;
use accroster_fake_mac::{FakeMac, FixtureLogin, Reply, StubCall};
use jiff::Timestamp;
use serde_json::json;

fn run(
    mac: &FakeMac,
    args: &[&str],
    extra_env: &[(&str, &str)],
    stdin: Option<&str>,
) -> accroster_core::SpawnOutput {
    let env = mac.environment();
    let mut vars = env.inherited_vars.clone();
    vars.extend(
        extra_env
            .iter()
            .map(|(n, v)| (OsString::from(n), OsString::from(v))),
    );
    env.spawner
        .run(&SpawnRequest {
            program: "claude".into(),
            args: args.iter().map(OsString::from).collect(),
            env: vars,
            cwd: None,
            stdin: stdin.map(|text| text.as_bytes().to_vec()),
        })
        .expect("the stub runs")
}

#[test]
fn auth_status_answers_from_its_canned_reply_and_records_the_call() {
    let mac = FakeMac::new();
    let status = json!({"loggedIn": true, "email": "me@example.com"});
    mac.claude.reply(StubCall::AuthStatus, Reply::json(&status));
    let folder = mac.path("profile one");

    let output = run(
        &mac,
        &["auth", "status", "--json"],
        &[("CLAUDE_CONFIG_DIR", folder.to_str().unwrap())],
        None,
    );

    assert!(output.succeeded());
    let printed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(printed, status);
    let calls = mac.claude.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].kind, StubCall::AuthStatus);
    assert_eq!(calls[0].args, ["auth", "status", "--json"]);
    assert_eq!(calls[0].var("CLAUDE_CONFIG_DIR"), folder.to_str());
    assert_eq!(calls[0].var("HOME"), mac.home().to_str());
}

#[test]
fn failures_and_non_zero_exits_are_replayed_in_order_and_the_last_reply_repeats() {
    let mac = FakeMac::new();
    mac.claude
        .reply(StubCall::GetUsage, Reply::fail(1, "network down"))
        .reply(StubCall::GetUsage, Reply::ok("not json at all\n"))
        .reply(
            StubCall::GetUsage,
            Reply::json(&json!({"rate_limits_available": true, "rate_limits": null})),
        );
    let args = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
    ];
    let request = "{\"type\":\"control_request\",\"request\":{\"subtype\":\"get_usage\"}}\n";

    let first = run(&mac, &args, &[], Some(request));
    let second = run(&mac, &args, &[], Some(request));
    let third = run(&mac, &args, &[], Some(request));
    let fourth = run(&mac, &args, &[], Some(request));

    assert_eq!(first.exit_code, Some(1));
    assert_eq!(String::from_utf8_lossy(&first.stderr), "network down");
    assert_eq!(String::from_utf8_lossy(&second.stdout), "not json at all\n");
    assert_eq!(
        third.stdout, fourth.stdout,
        "the last reply keeps answering"
    );
    let calls = mac.claude.calls();
    assert!(calls.iter().all(|call| call.kind == StubCall::GetUsage));
    assert_eq!(calls[0].stdin, request.trim_end());
}

#[test]
fn a_sign_in_reply_writes_its_files_into_the_config_folder() {
    let mac = FakeMac::new();
    let other = FixtureLogin::new("aaaa", "bbbb", "other@example.com", "Other Org");
    mac.claude.reply(
        StubCall::AuthLogin,
        Reply::ok("Login successful\n").writing(".claude.json", other.claude_json().to_string()),
    );
    let folder = mac.path("Library/Application Support/io.github.jbmanllr.accroster/profiles/p1");

    let output = run(
        &mac,
        &["auth", "login", "--claudeai"],
        &[("CLAUDE_CONFIG_DIR", folder.to_str().unwrap())],
        None,
    );

    assert!(output.succeeded());
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(folder.join(".claude.json")).unwrap())
            .unwrap();
    assert_eq!(written["oauthAccount"]["emailAddress"], "other@example.com");
}

#[test]
fn a_call_without_a_canned_reply_fails_loudly() {
    let mac = FakeMac::new();

    let output = run(&mac, &["auth", "logout"], &[], None);

    assert_eq!(output.exit_code, Some(97));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no canned reply for auth-logout"));
}

#[test]
fn inherited_variables_reach_the_stub_so_tests_can_check_what_was_cleared() {
    let mac = FakeMac::new();
    mac.inherit_var("CLAUDE_CODE_OAUTH_TOKEN", "inherited-token");
    mac.claude.reply(StubCall::SlashUsage, Reply::ok("usage\n"));

    run(&mac, &["-p", "/usage"], &[], None);

    let calls = mac.claude.calls();
    assert_eq!(calls[0].kind, StubCall::SlashUsage);
    assert_eq!(
        calls[0].var("CLAUDE_CODE_OAUTH_TOKEN"),
        Some("inherited-token")
    );
    assert!(
        calls[0]
            .var("PATH")
            .unwrap()
            .starts_with(mac.claude.bin_dir().to_str().unwrap())
    );
}

#[test]
fn the_fake_clock_and_os_facts_change_only_when_the_test_says_so() {
    let mac = FakeMac::new();
    let env = mac.environment();
    let started: Timestamp = "2026-10-25T13:00:00Z".parse().unwrap();

    assert_eq!(
        env.clock.now(),
        accroster_fake_mac::START_TIME.parse::<Timestamp>().unwrap()
    );
    mac.clock.advance(jiff::SignedDuration::from_mins(5));
    assert_eq!(
        env.clock.now(),
        "2026-10-25T14:07:00Z".parse::<Timestamp>().unwrap()
    );

    assert_eq!(env.os.process(4242), None);
    mac.os.start_process(4242, 1, started);
    assert_eq!(
        env.os.process(4242).map(|p| (p.parent_pid, p.started_at)),
        Some((1, started))
    );
    mac.os.end_process(4242);
    assert_eq!(env.os.process(4242), None);

    assert_eq!(env.os.desktop_app_version(), None);
    mac.os.set_desktop_app_version(Some("1.9.0"));
    assert_eq!(env.os.desktop_app_version().as_deref(), Some("1.9.0"));
}
