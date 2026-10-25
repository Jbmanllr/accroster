//! App storage: the app's own folder and settings.json.

use accroster_core::{Core, MenuItem};
use accroster_fake_mac::FakeMac;

const APP_FOLDER: &str = "Library/Application Support/io.github.jbmanllr.accroster";
const SETTINGS: &str = "Library/Application Support/io.github.jbmanllr.accroster/settings.json";

fn warnings(core: &Core) -> Vec<String> {
    core.view()
        .menu
        .items
        .into_iter()
        .filter_map(|item| match item {
            MenuItem::Warning(text) => Some(text),
            _ => None,
        })
        .collect()
}

#[test]
fn launch_creates_the_app_folder_with_every_subfolder_and_versioned_settings() {
    let mac = FakeMac::new();

    let _core = mac.launch();

    for folder in ["state", "index", "profiles", "bin", "logs", "backups"] {
        assert!(
            mac.path(APP_FOLDER).join(folder).is_dir(),
            "{folder}/ exists"
        );
    }
    let settings = mac.read_json(SETTINGS).expect("settings.json written");
    assert_eq!(settings["version"], 1);
}

#[test]
fn a_missing_settings_file_loads_defaults_without_a_warning() {
    let mac = FakeMac::new();

    let core = mac.launch();

    assert!(warnings(&core).is_empty());
    assert!(!core.view().settings.verbose_logging);
    assert!(
        mac.list(APP_FOLDER)
            .iter()
            .all(|name| !name.contains("corrupt"))
    );
}

#[test]
fn an_unparsable_settings_file_is_set_aside_defaults_load_and_the_app_warns_once() {
    for (case, contents) in [
        ("not JSON", "{\"version\": 1, \"verboseLogg"),
        ("unknown version", "{\"version\": 99}"),
        (
            "wrong shape",
            "{\"version\": 1, \"verboseLoggingUntil\": 42}",
        ),
        ("no version", "{}"),
    ] {
        let mac = FakeMac::new();
        mac.write_file(SETTINGS, contents);

        let core = mac.launch();

        let set_aside = format!("{APP_FOLDER}/settings.corrupt-20261025T140200Z.json");
        assert_eq!(
            mac.read_file(&set_aside).as_deref(),
            Some(contents),
            "{case}: original set aside"
        );
        let settings = mac.read_json(SETTINGS).expect("fresh settings written");
        assert_eq!(settings["version"], 1, "{case}: defaults written");
        assert!(
            !core.view().settings.verbose_logging,
            "{case}: defaults loaded"
        );
        let shown = warnings(&core);
        assert_eq!(shown.len(), 1, "{case}: one warning");
        assert!(
            shown[0].contains("settings.corrupt-20261025T140200Z.json"),
            "{case}: the warning names the set-aside file: {}",
            shown[0]
        );

        drop(core);
        let relaunched = mac.launch();
        assert!(
            warnings(&relaunched).is_empty(),
            "{case}: no warning on the next launch"
        );
    }
}
