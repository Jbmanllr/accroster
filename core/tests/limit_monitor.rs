//! The Limit monitor: readings that stay current on their own and are honest
//! about their age (cadence, menu-open refresh, sleep and lock, staleness,
//! offline, passed resets, backoff), all on the fake clock.

mod common;

use accroster_core::Event;
use accroster_fake_mac::{FakeMac, FixtureLogin, Reply, StubCall};
use common::*;
use jiff::SignedDuration;

/// A third Login, for tests with two Profiles.
fn team() -> FixtureLogin {
    FixtureLogin::new(
        "55555555-5555-4555-8555-555555555555",
        "66666666-6666-4666-8666-666666666666",
        "me@team.example",
        "Team Co",
    )
}

/// A Mac with two Profiles (work, then team) already added, whose next
/// usage reads answer `then`, in order (the last one repeats).
fn mac_with_two_profiles(then: Vec<Reply>) -> FakeMac {
    let mac = mac_signing_in_as(&work(), limits_body());
    mac.claude
        .reply(StubCall::AuthLogin, sign_in_as(&team()))
        .reply(StubCall::AuthStatus, auth_status(&team()))
        .reply(StubCall::GetUsage, usage_reply(limits_body()));
    for reply in then {
        mac.claude.reply(StubCall::GetUsage, reply);
    }
    let mut core = mac.launch();
    add_profile(&mut core);
    add_profile(&mut core);
    assert_eq!(rows(&core).len(), 2);
    mac
}

/// Sends a few ticks without waiting for reads to finish.
fn tick_a_few_times(core: &mut accroster_core::Core) {
    for _ in 0..5 {
        core.event(Event::Tick);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
fn reads_never_run_two_at_once() {
    let mac = mac_with_two_profiles(vec![Reply::hanging(), usage_reply(limits_body())]);

    // After a relaunch both Profiles are due at once.
    let mut core = mac.launch();
    core.event(Event::Tick);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while usage_reads(&mac) < 3 {
        assert!(
            std::time::Instant::now() < deadline,
            "the read never started"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    tick_a_few_times(&mut core);
    let reads: Vec<_> = mac
        .claude
        .calls()
        .into_iter()
        .filter(|call| call.kind == StubCall::GetUsage)
        .collect();
    assert_eq!(reads.len(), 3, "the second Profile waits for the first");
    assert!(reads[2].still_running());

    // A read that never answers is given up on after a while.
    pass(&mac, &mut core, SignedDuration::from_mins(2));
    assert!(!reads[2].still_running());
    assert!(
        mac.claude
            .calls()
            .iter()
            .all(|call| call.kind != StubCall::SlashUsage),
        "a hung read isn't retried through /usage"
    );
    assert_eq!(usage_reads(&mac), 4);
    let notes: Vec<_> = rows(&core).into_iter().map(|row| row.note).collect();
    assert_eq!(notes, [Some("Couldn't read limits".into()), None]);
}

#[test]
fn a_profile_that_is_not_active_is_read_again_every_15_minutes() {
    let mac = mac_signing_in_as(&work(), limits_body());
    let mut core = mac.launch();
    add_profile(&mut core);
    assert_eq!(usage_reads(&mac), 1);

    pass(&mac, &mut core, SignedDuration::from_secs(14 * 60 + 59));
    assert_eq!(usage_reads(&mac), 1);

    pass(&mac, &mut core, SignedDuration::from_secs(1));
    assert_eq!(usage_reads(&mac), 2);

    pass(&mac, &mut core, SignedDuration::from_mins(15));
    assert_eq!(usage_reads(&mac), 3);
}

#[test]
fn the_clis_active_profile_is_read_every_3_minutes() {
    let mac = mac_signing_in_as(&work(), limits_body());
    let mut core = mac.launch();
    add_profile(&mut core);
    make_cli_active(&mac);

    pass(&mac, &mut core, SignedDuration::from_secs(2 * 60 + 59));
    assert_eq!(usage_reads(&mac), 1);

    pass(&mac, &mut core, SignedDuration::from_secs(1));
    assert_eq!(usage_reads(&mac), 2);

    pass(&mac, &mut core, SignedDuration::from_mins(3));
    assert_eq!(usage_reads(&mac), 3);
}

/// A usage body with only Session and Weekly limits at these percentages.
fn body(session: u32, weekly: u32) -> serde_json::Value {
    serde_json::json!({
        "limits": [
            { "kind": "session", "percent": session, "resets_at": "2026-10-25T23:00:00Z" },
            { "kind": "weekly_all", "percent": weekly, "resets_at": "2026-10-30T09:00:00Z" },
        ],
    })
}

#[test]
fn opening_the_menu_shows_the_last_reading_then_refreshes_it_in_place() {
    let mac = mac_signing_in_as(&work(), body(62, 41));
    mac.claude
        .reply(StubCall::GetUsage, usage_reply(body(70, 43)));
    let mut core = mac.launch();
    add_profile(&mut core);

    // A reading a minute old is left alone.
    mac.clock.advance(SignedDuration::from_secs(60));
    core.event(Event::MenuOpened);
    settle(&mut core);
    assert_eq!(usage_reads(&mac), 1);

    mac.clock.advance(SignedDuration::from_secs(1));
    core.event(Event::MenuOpened);
    assert_eq!(
        rows(&core)[0].limits,
        [bar("Session", 62), bar("Weekly", 41)],
        "the menu opens on the last reading"
    );
    settle(&mut core);
    assert_eq!(usage_reads(&mac), 2);
    assert_eq!(
        rows(&core)[0].limits,
        [bar("Session", 70), bar("Weekly", 43)]
    );
}

/// The Profile folder signed in as `login`.
fn folder_of(mac: &FakeMac, login: &FixtureLogin) -> std::path::PathBuf {
    mac.list(PROFILES)
        .into_iter()
        .map(|id| mac.path(format!("{PROFILES}/{id}")))
        .find(|folder| {
            std::fs::read_to_string(folder.join(".claude.json"))
                .is_ok_and(|text| text.contains(&login.email))
        })
        .expect("a Profile folder for this Login")
}

/// The Profile folders of the usage reads so far, in order.
fn read_folders(mac: &FakeMac) -> Vec<std::path::PathBuf> {
    mac.claude
        .calls()
        .into_iter()
        .filter(|call| call.kind == StubCall::GetUsage)
        .map(|call| call.var("CLAUDE_CONFIG_DIR").expect("set").into())
        .collect()
}

#[test]
fn nothing_is_read_while_the_mac_sleeps() {
    let mac = mac_signing_in_as(&work(), limits_body());
    let mut core = mac.launch();
    add_profile(&mut core);

    core.event(Event::WillSleep);
    pass(&mac, &mut core, SignedDuration::from_hours(1));
    assert_eq!(usage_reads(&mac), 1);

    core.event(Event::DidWake);
    settle(&mut core);
    assert_eq!(usage_reads(&mac), 2);
}

#[test]
fn nothing_is_read_while_the_screen_is_locked() {
    let mac = mac_signing_in_as(&work(), limits_body());
    let mut core = mac.launch();
    add_profile(&mut core);

    core.event(Event::ScreenLocked);
    pass(&mac, &mut core, SignedDuration::from_hours(1));
    assert_eq!(usage_reads(&mac), 1);

    core.event(Event::ScreenUnlocked);
    settle(&mut core);
    assert_eq!(usage_reads(&mac), 2);
}

#[test]
fn on_wake_and_unlock_the_active_profile_is_read_first() {
    let mac = mac_with_two_profiles(vec![]);
    let team_folder = folder_of(&mac, &team());
    mac.write_file(
        format!("{APP_FOLDER}/bin/cli-active-profile"),
        team_folder.to_str().unwrap(),
    );
    let mut core = mac.launch();
    settle(&mut core);
    let before = usage_reads(&mac);

    core.event(Event::WillSleep);
    mac.clock.advance(SignedDuration::from_hours(1));
    core.event(Event::DidWake);
    settle(&mut core);
    let folders = read_folders(&mac);
    assert_eq!(folders.len(), before + 2, "both are overdue");
    assert_eq!(folders[before], team_folder);

    // Unlocking soon after: only the Active Profile is read again at once.
    core.event(Event::ScreenLocked);
    mac.clock.advance(SignedDuration::from_mins(2));
    core.event(Event::ScreenUnlocked);
    settle(&mut core);
    let folders = read_folders(&mac);
    assert_eq!(folders.len(), before + 3);
    assert_eq!(folders[before + 2], team_folder);
}

/// The only Profile row's hover-submenu detail lines.
fn first_details(core: &accroster_core::Core) -> Vec<String> {
    details(&rows(core)[0])
}

#[test]
fn a_reading_older_than_twice_its_cadence_is_dimmed_with_its_time() {
    let mac = mac_signing_in_as(&work(), limits_body());
    let mut core = mac.launch();
    add_profile(&mut core);
    // Asleep, so nothing refreshes it.
    core.event(Event::WillSleep);

    pass(&mac, &mut core, SignedDuration::from_mins(30));
    let row = rows(&core).remove(0);
    assert!(!row.dimmed);
    assert_eq!(row.note, None);

    pass(&mac, &mut core, SignedDuration::from_secs(1));
    let row = rows(&core).remove(0);
    assert!(row.dimmed);
    assert_eq!(row.note.as_deref(), Some("as of 14:02"));
    assert_eq!(
        row.limits,
        [bar("Session", 62), bar("Weekly", 41), bar("Fable", 92)],
        "the old figures stay"
    );
}

#[test]
fn an_active_profiles_reading_is_stale_after_6_minutes() {
    let mac = mac_signing_in_as(&work(), limits_body());
    let mut core = mac.launch();
    add_profile(&mut core);
    make_cli_active(&mac);
    core.event(Event::WillSleep);

    pass(&mac, &mut core, SignedDuration::from_mins(6));
    assert!(!rows(&core)[0].dimmed);
    pass(&mac, &mut core, SignedDuration::from_secs(1));
    assert!(rows(&core)[0].dimmed);
    assert_eq!(rows(&core)[0].note.as_deref(), Some("as of 14:02"));
}

/// A usage read that also leaves Claude Code's cache in `.claude.json`,
/// fetched at `fetched_at` for the Login `account_uuid`.
fn usage_reply_cached(account_uuid: &str, fetched_at: &str) -> Reply {
    let fetched_at: jiff::Timestamp = fetched_at.parse().unwrap();
    let mut claude_json = work().claude_json();
    claude_json["cachedUsageUtilization"] = serde_json::json!({
        "fetchedAtMs": fetched_at.as_millisecond(),
        "accountUuid": account_uuid,
        "utilization": limits_body(),
    });
    usage_reply(limits_body()).writing(".claude.json", claude_json.to_string())
}

#[test]
fn the_reading_time_is_claude_codes_fetch_time_for_this_login() {
    let mac = mac_signing_in_as(&work(), limits_body());
    mac.claude.reply(
        StubCall::GetUsage,
        usage_reply_cached(&work().account_uuid, "2026-10-25T13:50:00Z"),
    );
    let mut core = mac.launch();
    add_profile(&mut core);
    assert!(first_details(&core).contains(&"Limits as of 14:02".to_string()));

    pass(&mac, &mut core, SignedDuration::from_mins(15));
    assert!(
        first_details(&core).contains(&"Limits as of 13:50".to_string()),
        "{:?}",
        first_details(&core)
    );
}

#[test]
fn another_logins_cached_fetch_time_is_ignored() {
    let mac = mac_signing_in_as(&work(), limits_body());
    mac.claude.reply(
        StubCall::GetUsage,
        usage_reply_cached(&team().account_uuid, "2026-10-25T13:50:00Z"),
    );
    let mut core = mac.launch();
    add_profile(&mut core);

    pass(&mac, &mut core, SignedDuration::from_mins(15));
    assert!(
        first_details(&core).contains(&"Limits as of 14:17".to_string()),
        "{:?}",
        first_details(&core)
    );
}

#[test]
fn offline_polling_stops_and_one_line_tops_the_menu() {
    let mac = mac_signing_in_as(&work(), limits_body());
    let mut core = mac.launch();
    add_profile(&mut core);

    core.event(Event::NetworkChanged { online: false });
    pass(&mac, &mut core, SignedDuration::from_hours(1));
    assert_eq!(usage_reads(&mac), 1);
    let items = core.view().menu.items;
    assert_eq!(
        items[0],
        accroster_core::MenuItem::Warning("Offline — showing figures from 14:02".into())
    );
    assert_eq!(
        warnings(&core).len(),
        1,
        "one line, however many Profiles: {items:?}"
    );

    core.event(Event::NetworkChanged { online: true });
    settle(&mut core);
    assert_eq!(usage_reads(&mac), 2);
    assert!(warnings(&core).is_empty());
}

#[test]
fn opening_the_menu_offline_reads_nothing() {
    let mac = mac_signing_in_as(&work(), limits_body());
    let mut core = mac.launch();
    add_profile(&mut core);
    core.event(Event::NetworkChanged { online: false });

    mac.clock.advance(SignedDuration::from_mins(5));
    core.event(Event::MenuOpened);
    settle(&mut core);
    assert_eq!(usage_reads(&mac), 1);
}

/// A failed fetch: limits are available to this Login, but Claude Code got
/// none.
fn failed_fetch() -> Reply {
    usage_reply(serde_json::Value::Null)
}

/// Lets the clock run to `hh:mm` UTC on 25 October, then lets the core act.
fn at(mac: &FakeMac, core: &mut accroster_core::Core, hh_mm: &str) {
    let target: jiff::Timestamp = format!("2026-10-25T{hh_mm}:00Z").parse().unwrap();
    mac.clock.set(target);
    settle(core);
}

#[test]
fn failed_fetches_back_off_from_5_minutes_doubling_to_an_hour() {
    let mac = mac_signing_in_as(&work(), body(62, 41));
    for _ in 0..6 {
        mac.claude.reply(StubCall::GetUsage, failed_fetch());
    }
    mac.claude
        .reply(StubCall::GetUsage, usage_reply(body(70, 43)));
    let mut core = mac.launch();
    add_profile(&mut core);

    at(&mac, &mut core, "14:17");
    assert_eq!(usage_reads(&mac), 2);
    let row = rows(&core).remove(0);
    assert_eq!(row.limits, [bar("Session", 62), bar("Weekly", 41)]);
    assert_eq!(
        row.note.as_deref(),
        Some("Couldn't refresh · next try 14:22")
    );
    assert!(first_details(&core).contains(&"Couldn't refresh · next try 14:22".to_string()));

    at(&mac, &mut core, "14:21");
    assert_eq!(usage_reads(&mac), 2);
    // 5, 10, 20, 40, then capped at 60 minutes.
    for (now, next) in [
        ("14:22", "14:32"),
        ("14:32", "14:52"),
        ("14:52", "15:32"),
        ("15:32", "16:32"),
        ("16:32", "17:32"),
    ] {
        at(&mac, &mut core, now);
        assert_eq!(
            rows(&core)[0].note,
            Some(format!("Couldn't refresh · next try {next}")),
            "after the read at {now}"
        );
    }
    assert_eq!(usage_reads(&mac), 7);

    // Success clears the backoff; the 15-minute cadence resumes.
    at(&mac, &mut core, "17:32");
    assert_eq!(usage_reads(&mac), 8);
    let row = rows(&core).remove(0);
    assert_eq!(row.limits, [bar("Session", 70), bar("Weekly", 43)]);
    assert_eq!(row.note, None);
    at(&mac, &mut core, "17:47");
    assert_eq!(usage_reads(&mac), 9);
}

#[test]
fn backoff_survives_a_restart() {
    let mac = mac_signing_in_as(&work(), body(62, 41));
    mac.claude.reply(StubCall::GetUsage, failed_fetch());
    let mut core = mac.launch();
    add_profile(&mut core);
    at(&mac, &mut core, "14:17");
    drop(core);

    let mut core = mac.launch();
    at(&mac, &mut core, "14:21");
    assert_eq!(usage_reads(&mac), 2, "nothing is read before the next try");
    assert_eq!(
        rows(&core)[0].note.as_deref(),
        Some("Couldn't refresh · next try 14:22")
    );
    at(&mac, &mut core, "14:22");
    assert_eq!(usage_reads(&mac), 3);
    assert_eq!(
        rows(&core)[0].note.as_deref(),
        Some("Couldn't refresh · next try 14:32"),
        "the count carried over"
    );
}

#[test]
fn opening_the_menu_skips_a_profile_in_backoff() {
    let mac = mac_signing_in_as(&work(), body(62, 41));
    mac.claude.reply(StubCall::GetUsage, failed_fetch());
    let mut core = mac.launch();
    add_profile(&mut core);
    at(&mac, &mut core, "14:17");

    at(&mac, &mut core, "14:19");
    core.event(Event::MenuOpened);
    settle(&mut core);
    assert_eq!(usage_reads(&mac), 2);
}

#[test]
fn a_fetch_that_fails_as_the_network_goes_away_does_not_count() {
    let mac = mac_signing_in_as(&work(), body(62, 41));
    mac.claude
        .reply(StubCall::GetUsage, failed_fetch())
        .reply(StubCall::GetUsage, usage_reply(body(70, 43)));
    let mut core = mac.launch();
    add_profile(&mut core);

    mac.clock.set("2026-10-25T14:17:00Z".parse().unwrap());
    core.event(Event::Tick); // starts the read
    core.event(Event::NetworkChanged { online: false });
    settle(&mut core);
    assert_eq!(usage_reads(&mac), 2);
    assert!(
        !first_details(&core)
            .iter()
            .any(|line| line.starts_with("Couldn't refresh")),
        "{:?}",
        first_details(&core)
    );

    // Back online, it is retried as soon as a minute has passed, not after
    // a backoff or the full cadence.
    mac.clock.advance(SignedDuration::from_secs(60));
    core.event(Event::NetworkChanged { online: true });
    settle(&mut core);
    assert_eq!(usage_reads(&mac), 3);
    assert_eq!(
        rows(&core)[0].limits,
        [bar("Session", 70), bar("Weekly", 43)]
    );
}

#[test]
fn an_unreadable_answer_keeps_the_dimmed_reading_and_does_not_back_off() {
    let mac = mac_signing_in_as(&work(), body(62, 41));
    mac.claude
        .reply(
            StubCall::GetUsage,
            usage_reply(serde_json::json!({ "something": "else" })),
        )
        .reply(StubCall::GetUsage, usage_reply(body(70, 43)));
    let mut core = mac.launch();
    add_profile(&mut core);

    at(&mac, &mut core, "14:17");
    let row = rows(&core).remove(0);
    assert_eq!(row.limits, [bar("Session", 62), bar("Weekly", 41)]);
    assert!(row.dimmed);
    assert_eq!(row.note.as_deref(), Some("Couldn't read limits"));
    assert!(
        !first_details(&core)
            .iter()
            .any(|line| line.starts_with("Couldn't refresh")),
        "{:?}",
        first_details(&core)
    );

    // The next read comes at the usual cadence, not after a backoff.
    at(&mac, &mut core, "14:31");
    assert_eq!(usage_reads(&mac), 2);
    at(&mac, &mut core, "14:32");
    assert_eq!(usage_reads(&mac), 3);
    assert!(!rows(&core)[0].dimmed);
}

/// Session 62% resetting at 14:10, Weekly 41% much later.
fn body_resetting_at_14_10() -> serde_json::Value {
    serde_json::json!({
        "limits": [
            { "kind": "session", "percent": 62, "resets_at": "2026-10-25T14:10:00Z" },
            { "kind": "weekly_all", "percent": 41, "resets_at": "2026-10-30T09:00:00Z" },
        ],
    })
}

fn reset_bar(label: &str) -> accroster_core::LimitBar {
    accroster_core::LimitBar {
        label: label.into(),
        percent: 0,
        reset: true,
    }
}

#[test]
fn a_passed_reset_shows_reset_and_refreshes_at_once() {
    let mac = mac_signing_in_as(&work(), body_resetting_at_14_10());
    mac.claude
        .reply(StubCall::GetUsage, usage_reply(body(3, 42)));
    let mut core = mac.launch();
    add_profile(&mut core);
    core.event(Event::WillSleep);

    at(&mac, &mut core, "14:09");
    assert_eq!(
        rows(&core)[0].limits,
        [bar("Session", 62), bar("Weekly", 41)]
    );

    at(&mac, &mut core, "14:10");
    assert_eq!(
        rows(&core)[0].limits,
        [reset_bar("Session"), bar("Weekly", 41)]
    );
    assert_eq!(usage_reads(&mac), 1, "asleep: shown as reset, not read");

    core.event(Event::DidWake);
    settle(&mut core);
    assert_eq!(usage_reads(&mac), 2);
    assert_eq!(
        rows(&core)[0].limits,
        [bar("Session", 3), bar("Weekly", 42)]
    );
}

#[test]
fn a_reset_answered_from_an_old_cache_is_not_refreshed_again_and_again() {
    let mac = mac_signing_in_as(&work(), body_resetting_at_14_10());
    let mut core = mac.launch();
    add_profile(&mut core);

    at(&mac, &mut core, "14:10");
    assert_eq!(usage_reads(&mac), 2);
    at(&mac, &mut core, "14:16");
    assert_eq!(usage_reads(&mac), 2, "next at the usual cadence");
    at(&mac, &mut core, "14:25");
    assert_eq!(usage_reads(&mac), 3);
}

#[test]
fn a_passed_reset_waits_for_the_backoff() {
    let mac = mac_signing_in_as(&work(), body_resetting_at_14_10());
    mac.claude.reply(StubCall::GetUsage, failed_fetch());
    let mut core = mac.launch();
    add_profile(&mut core);
    // Opening the menu at 14:09 reads, and fails: next try 14:14.
    at(&mac, &mut core, "14:09");
    core.event(Event::MenuOpened);
    settle(&mut core);
    assert_eq!(
        rows(&core)[0].note.as_deref(),
        Some("Couldn't refresh · next try 14:14")
    );

    at(&mac, &mut core, "14:10");
    assert_eq!(rows(&core)[0].limits[0], reset_bar("Session"));
    assert_eq!(usage_reads(&mac), 2);
    at(&mac, &mut core, "14:14");
    assert_eq!(usage_reads(&mac), 3);
}

#[test]
fn one_profile_is_never_read_twice_within_a_minute() {
    // Claude Code answers from a cache fetched 12 minutes earlier, so the
    // reading is already old when it lands.
    let mac = mac_signing_in_as(&work(), limits_body());
    mac.claude.reply(
        StubCall::GetUsage,
        usage_reply_cached(&work().account_uuid, "2026-10-25T13:50:00Z"),
    );
    let mut core = mac.launch();
    add_profile(&mut core);
    pass(&mac, &mut core, SignedDuration::from_mins(15));
    assert_eq!(usage_reads(&mac), 2);

    mac.clock.advance(SignedDuration::from_secs(59));
    core.event(Event::MenuOpened);
    settle(&mut core);
    assert_eq!(
        usage_reads(&mac),
        2,
        "the reading is old, but the read was not"
    );

    pass(&mac, &mut core, SignedDuration::from_secs(1));
    assert_eq!(usage_reads(&mac), 3, "the requested refresh follows");
}

#[test]
fn the_menu_bar_figure_dims_with_the_clis_active_profiles_reading() {
    let mac = mac_signing_in_as(&work(), limits_body());
    let mut core = mac.launch();
    add_profile(&mut core);
    core.command(accroster_core::Command::SetCliSwitching(true));
    let use_it = rows(&core)[0].command.clone().expect("a row click");
    core.command(use_it);
    core.event(Event::WillSleep);

    pass(&mac, &mut core, SignedDuration::from_mins(6));
    let menu_bar = core.view().menu_bar;
    assert_eq!(menu_bar.label, "me 92%");
    assert!(!menu_bar.dimmed);

    pass(&mac, &mut core, SignedDuration::from_secs(1));
    let menu_bar = core.view().menu_bar;
    assert_eq!(menu_bar.label, "me 92%");
    assert!(menu_bar.dimmed);
}
