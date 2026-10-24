# Rust stack for a lightweight macOS menu bar app

Which Rust stack fits a menu bar app that shows short text in the status item (`W·42%`), lists Profiles with two usage bars each plus a list of Sessions, sends notifications, launches at login, reads and writes the Keychain, watches `~/.claude` and `~/Library/Application Support/Claude`, and polls HTTPS endpoints. Answers [#9](https://github.com/Jbmanllr/claude-account-manager/issues/9) for the map [#1](https://github.com/Jbmanllr/claude-account-manager/issues/1).

Researched 2026-10-24 on macOS 27.0.1, Apple M3 Max, rustc 1.93.0. Crate versions are the latest on crates.io that day unless noted.

Labels: **Measured** (my prototype on this Mac), **Documented** (official docs, crate docs or source), **Issue report** (a GitHub issue, not confirmed by the maintainer), **Inference** (my reasoning).

## Summary

- **Default: direct AppKit through `objc2` / `objc2-app-kit`.** It is the only option that gives a native `NSMenu` whose rows are custom views (a label plus two `NSProgressIndicator` bars) and a native `NSPopover`, at the smallest cost: 312 KiB binary, 12 MB idle footprint (**Measured**). Everything else in this list is built on `objc2` already.
- **Runner-up: Tauri v2 with the `tray-icon` feature plus `tauri-nspanel` or `tauri-plugin-positioner`.** Best if the Unified session view grows into a searchable window and web UI speed matters more than weight. Cost: 3.9 MiB binary, about 23 MB in the app process plus about 30 MB in three WebKit helper processes (**Measured**).
- **`tray-icon` + `muda` alone cannot draw the usage bars.** `muda` menus only have text, check, icon, predefined and submenu items (**Documented**, source). It is light (818 KiB, 12 MB) and exposes the underlying `NSStatusItem`, but once you need custom views you are writing `objc2` code anyway.
- **egui/eframe has no status item and the current release does not build on the local toolchain.** eframe 0.36+ requires rustc 1.95; 0.35 builds. A wgpu window idles at about 41 MB (**Measured**); the tray comes from `tray-icon`.
- **Slint 1.17+ has a native `SystemTrayIcon` with a menu bar text title**, but its tray menu is text only and the popover is a self-drawn window. 8.2 MiB binary, 22 MB tray only, 34 MB with a window (**Measured**). GPL-3.0 or royalty-free licence with conditions.
- **Notifications and launch at login need a `.app` bundle with a bundle identifier** whatever the UI stack (`UNUserNotificationCenter`, `SMAppService.mainApp`). An ad-hoc signature is enough for notifications. Plan a tiny bundling step even for personal use. Notarization stays out of scope.
- **Keychain prompts follow the code signature.** An ad-hoc signed build gets a new identity on every rebuild, so "Always Allow" on Claude Code's Keychain item has to be granted again after each rebuild (**Issue report** + **Inference**). A stable local self-signed signing identity avoids it.

## Measurements

Each prototype shows `W·42%` in the menu bar with the dock icon hidden (accessory activation policy). Built with `--release`, `strip = true`, `lto = true`, `codegen-units = 1`, `panic = "abort"`. Memory sampled 20 s after launch with `footprint -p` (physical footprint, what Activity Monitor reports as Memory) and `ps -o rss` (RSS also counts shared system frameworks, so it is always higher). Three runs per row, plus two earlier 10 s runs; values were stable except where noted. Idle CPU was 0.0 % for all at sampling time.

| Stack | Prototype content | Binary | Idle footprint | Extra processes |
|---|---|--:|--:|---|
| objc2 0.6.5 + objc2-app-kit 0.3.2 | Status item; `NSMenu` with 2 custom-view rows (label + 2 `NSProgressIndicator`) and Quit | **312 KiB** | **12 MB** | none |
| tray-icon 0.26.1 + tao 0.37.2 (muda 0.21) | Status item; text-only menu | 818 KiB | 12 MB | none |
| eframe 0.35.0 (wgpu, default features) | One 300×160 window with 4 progress bars, no tray | 7.3 MiB | 40–42 MB | none |
| Slint 1.18.1 (winit + femtovg) | `SystemTrayIcon` with title and menu, window hidden | 8.2 MiB | 22–23 MB | none |
| Slint 1.18.1 | Same, window with 4 progress bars shown | 8.2 MiB | 34–35 MB | none |
| Tauri 2.12.5 (`tray-icon` feature) | Tray with title and menu; webview window created hidden | 3.9 MiB | 23–24 MB | WebKit GPU 10–12 MB, WebContent 11–13 MB, Networking 6–7 MB (≈ 30 MB) |
| Tauri 2.12.5 | Same, window shown | 3.9 MiB | 23–24 MB | same three, ≈ 30 MB |

Notes:

- One eframe run peaked at 376 MB footprint 10 s after launch before settling at about 41 MB in later runs; treat wgpu start-up as spiky.
- The Tauri helper processes are WebKit XPC services started for the app's `WKWebView`; they are not children of the app's PID, so a per-process view (Activity Monitor's row for the app) understates Tauri's total.
- A menu bar screenshot confirmed the text titles render. I did not open the objc2 menu to inspect the custom-view rows visually; the code compiles and runs.
- Prototype sources were kept in a scratch directory and are not committed. Build times from a warm registry: objc2 2 s, tray-icon 13 s, eframe 40 s, Slint 1 min 37 s, Tauri 1 min 55 s.

## Option by option

### 1. Direct AppKit via objc2 (`objc2`, `objc2-foundation`, `objc2-app-kit`)

- **What it gives.** Generated bindings for every AppKit class, including `NSStatusItem`, `NSStatusBarButton`, `NSMenuItem` (with `setView`), `NSProgressIndicator`, `NSLevelIndicator` and `NSPopover` (**Documented**: `objc2-app-kit` 0.3.2 source has `generated/NSPopover.rs`, `NSStatusItem.rs`, `NSMenuItem.rs`, `NSProgressIndicator.rs`, `NSLevelIndicator.rs`). Target/action handlers and delegates are declared with the `define_class!` macro ([objc2 docs](https://docs.rs/objc2/latest/objc2/macro.define_class.html)). Main-thread-only classes need a `MainThreadMarker`, and `alloc` comes from the `MainThreadOnly` trait (**Measured**: compile errors until imported).
- **Rich menu.** Custom-view menu rows with real `NSProgressIndicator` bars work today (prototype). A popover (`NSPopover` anchored to the status item button) is the alternative if the Sessions list gets long.
- **Native look.** Fully native: system font, dark mode, accessibility and menu behaviour come from AppKit.
- **Background work.** Polling and file watching run on worker threads; UI updates go back to the main thread with `dispatch2` (0.3.1, same author) or `NSRunLoop` timers (**Inference**).
- **Maturity and maintenance.** `objc2` is the base layer of winit, tao, wry, tray-icon and muda: 142 M downloads, 56 M in the last 90 days ([crates.io](https://crates.io/crates/objc2)). Main branch was updated to Xcode 27.1 on 2026-10-16 ([commits](https://github.com/madsmtm/objc2/commits)). Framework crates are pre-1.0 (0.2 → 0.3 was breaking) and the latest framework release is 0.3.2 from 2025-10-04; `objc2` itself shipped 0.6.5 on 2026-10-05 ([crates.io versions](https://crates.io/crates/objc2-app-kit/versions)).
- **Costs.** More code than a declarative UI; some calls are `unsafe` (the prototype needed one `unsafe` block for `initWithTitle:action:keyEquivalent:`); layout is manual frames or Auto Layout. macOS only, which matches the project.
- **Companion crates.** `objc2-user-notifications` (0.3.2) for `UNUserNotificationCenter`, `objc2-service-management` (0.3.2) for `SMAppService`, both from the same repository.

### 2. tray-icon + muda (+ tao or winit)

- **What it gives.** A cross-platform status item with icon, `with_title` / `set_title` text, tooltip and a `muda` menu ([tray-icon source](https://github.com/tauri-apps/tray-icon/blob/dev/src/lib.rs)). On macOS "an event loop must be running on the main thread", and the icon must be created after the loop starts, for example at `StartCause::Init` (**Documented**, crate docs).
- **Rich menu.** None. `muda::MenuItemKind` is `MenuItem`, `Submenu`, `Predefined`, `Check` or `Icon` ([muda source](https://github.com/tauri-apps/muda/blob/dev/src/items/mod.rs)). Usage could only be shown as text (`S 30%  W 42%`) or as an icon image you render yourself.
- **Escape hatches.** `TrayIcon::ns_status_item()` returns the `objc2_app_kit::NSStatusItem` and `TrayIcon::rect()` gives its position, so a popover or custom `NSMenu` can be attached with `objc2`; `muda` exposes `ns_menu()` as a raw pointer (**Documented**, source). At that point the app depends on `objc2` code anyway and the cross-platform layer buys nothing for a macOS-only app (**Inference**).
- **Popover.** No built-in popover; either a tao window you draw into (no widget toolkit) or `NSPopover` via the escape hatch.
- **Maturity.** Maintained by the Tauri team and used inside Tauri: tray-icon 0.26.1 (2026-10-07), muda 0.21.2 (2026-10-08), tao 0.37.2 (2026-10-23); 16–18 M downloads in 90 days each. tray-icon has 49 open issues. MSRV 1.90.

### 3. Tauri v2 tray + popover window

- **What it gives.** `TrayIconBuilder` with `title` (shown next to the icon on macOS), menus, and tray events ([Tauri system tray guide](https://v2.tauri.app/learn/system-tray/), [TrayIconBuilder docs](https://docs.rs/tauri/latest/tauri/tray/struct.TrayIconBuilder.html)). `app.set_activation_policy(ActivationPolicy::Accessory)` hides the dock icon (**Measured**, prototype).
- **Popover approaches.**
  - `tauri-plugin-positioner` (2.4.0, official) moves a window to `TrayCenter`, `TrayBottomCenter` and similar positions once `on_tray_event` is wired, and constrains it to the screen ([positioner docs](https://v2.tauri.app/plugin/positioner/)). Show on tray click, hide on blur.
  - `tauri-nspanel` (2.1.1, 2026-10-08, 427 stars) converts the window into an `NSPanel`, which can be non-activating and float over full-screen apps ([README](https://github.com/ahkohd/tauri-nspanel)). A third-party write-up confirms a plain Tauri window is invisible over full-screen apps without it ([DEV article](https://dev.to/manasightgg/why-i-chose-tauri-v2-for-a-desktop-overlay-in-2026-597h), secondary).
  - `tauri-plugin-nspopover` wraps a native `NSPopover`; git-only install, no releases, last commit 2026-05-16, still depends on the deprecated `cocoa`/`objc` 0.2 crates next to `objc2` ([repo](https://github.com/freethinkel/tauri-nspopover-plugin)). Listed as a community plugin on [v2.tauri.app/plugin](https://v2.tauri.app/plugin).
- **Rich content.** Anything HTML can draw; progress bars are `<progress>`. Menus are still `muda` menus (text only).
- **Native look.** Web UI styled to look native; the tray menu itself is native.
- **Weight.** Heaviest option measured: one app process plus three WebKit services (≈ 53 MB total). The webview processes stay alive while the window is merely hidden.
- **Plugins.** `tauri-plugin-notification` 2.7.0 uses `notify-rust` on macOS ([Cargo.toml](https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/notification/Cargo.toml)); `tauri-plugin-autostart` 2.7.0 wraps `auto-launch`, which uses a LaunchAgent plist or AppleScript login item, not `SMAppService` ([auto-launch docs](https://docs.rs/auto-launch/0.6.0/auto_launch/)).
- **Maturity.** Tauri 2.12.5 (2026-10-21), 112 k stars, very active, 1 481 open issues. MSRV 1.90.

### 4. egui / eframe

- **Status item.** None in egui or eframe; the maintainer answer is to use a third-party crate ([discussion #1388](https://github.com/emilk/egui/discussions/1388)). `tray-icon` ships an eframe example ([examples/egui.rs](https://github.com/tauri-apps/tray-icon/blob/dev/examples/egui.rs)).
- **Popover.** A borderless eframe window positioned under the tray rect. Hiding and re-showing a window is a known weak spot: eframe cannot run on an existing event loop ([#2875](https://github.com/emilk/egui/issues/2875) referenced in [discussion #737](https://github.com/emilk/egui/discussions/737)), and on Windows a hidden viewport could not be shown again ([#5229](https://github.com/emilk/egui/issues/5229)). Hidden windows skip egui passes to save CPU ([#8671](https://github.com/emilk/egui/issues/8671), [winit_integration.rs](https://github.com/emilk/egui/blob/main/crates/eframe/src/native/winit_integration.rs)).
- **Native look.** No; egui draws its own immediate-mode widgets.
- **Toolchain.** eframe 0.36.0+ declares `rust-version = 1.95`; local rustc 1.93 refuses it, eframe 0.35.0 (MSRV 1.92) builds (**Measured**, [crates.io versions](https://crates.io/crates/eframe/versions)).
- **Weight.** 7.3 MiB binary and about 41 MB idle with the default wgpu renderer (**Measured**).
- **Maturity.** Very active (31.7 k stars, 0.37.0 on 2026-10-15), frequent breaking releases.

### 5. Other Rust menu bar frameworks found

- **Slint 1.17+ `SystemTrayIcon`** ([docs](https://docs.slint.dev/latest/docs/slint/reference/window/systemtrayicon), [1.17 release](https://slint.dev/blog/slint-1.17-released)): `NSStatusItem` on macOS, `title` is "the visible text label rendered next to the icon in the menu bar". The tray must contain exactly one `Menu` of `MenuItem` / `MenuSeparator` (text only); rich content lives in a separate Slint window, drawn by Slint, not AppKit. Licence `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0` ([crates.io](https://crates.io/crates/slint)). MSRV 1.92. Measured 22 MB tray only, 34 MB with a window.
- **`tray`** (0.1.2, Dec 2025, "with egui and iced support") ([lib.rs](https://lib.rs/crates/tray)): young, three releases, no text title mentioned.
- **`cacao`** (AppKit wrapper, 0.3.2 from 2023-08, last push 2025-02) and **`menubar`** (0.0.2, 2021) and **`tray-item`** (0.10.0, 2024-04): stale; not recommended.
- **fastframe** (`fastframe-tray`, `fastframe-shell`) wraps eframe for tray-first apps ([fastframe.dev](https://fastframe.dev/)); small project, same egui trade-offs.

## Capability crates

| Need | Recommended | Alternatives and notes |
|---|---|---|
| Keychain read/write | `security-framework` 3.7.0: `passwords::get_generic_password(service, account)`, `set_generic_password`, `delete_generic_password` ([docs](https://docs.rs/security-framework/latest/security_framework/passwords/index.html)). 103 M downloads in 90 days. | `keyring` 4.2.0 is now a wrapper: the `v1` feature keeps the old API on top of `keyring-core` plus per-platform stores such as `apple-native-keyring-store` ([docs](https://docs.rs/keyring/4.2.0/keyring/index.html), [README](https://github.com/open-source-cooperative/keyring-rs)). Extra layers for a macOS-only app. The `apple-native-keyring-store` README says to use its `keychain` module for apps not signed with a provisioning profile ([crates.io](https://crates.io/crates/apple-native-keyring-store)). |
| Notifications | objc2 path: `objc2-user-notifications` 0.3.2 (`UNUserNotificationCenter`). Other paths: `notify-rust` 4.18.2 with the `preview-macos-un` feature, which switches to `mac-usernotifications` ([features](https://docs.rs/crate/notify-rust/4.18.2/features)). | Default `notify-rust` on macOS uses `mac-notification-sys`, built on `NSUserNotification` ([platform table](https://docs.rs/notify-rust/4.18.2/notify_rust/index.html)), which Apple deprecated ([Apple](https://developer.apple.com/documentation/foundation/nsusernotification)). `UNUserNotificationCenter` needs a `.app` bundle with `CFBundleIdentifier`, code-signed, ad-hoc is enough ([mac-usernotifications docs](https://docs.rs/mac-usernotifications)). `user-notify` 0.4.2 is another small wrapper. |
| Launch at login | `SMAppService.mainApp.register()` via `objc2-service-management` 0.3.2 (macOS 13+) ([Apple](https://developer.apple.com/documentation/servicemanagement/smappservice/mainapp)). | `smappservice-rs` 0.1.3 wraps the same API (last release 2025-05, 12 stars) ([crates.io](https://crates.io/crates/smappservice-rs)). `auto-launch` 0.6.0 / `tauri-plugin-autostart` write a LaunchAgent plist or use AppleScript; works without `SMAppService` but shows as a generic background item. `mainApp` refers to the app bundle, so it needs the `.app` too (**Inference** from Apple's description). |
| File watching | `notify` 8.2.0, FSEvents backend by default on macOS (`macos_fsevent`), kqueue optional ([docs](https://docs.rs/notify/8.2.0/notify/)). Add a debouncer (`notify-debouncer-full`) for bursty writes. | FSEvents may miss events on files the user does not own; not an issue for `~/.claude`. |
| HTTPS polling | `ureq` 3.5.0: blocking, no async runtime, `rustls` with `ring` by default, forbids `unsafe` ([docs](https://docs.rs/ureq/3.5.0/ureq/)). Fits a few requests per minute on a worker thread. | `reqwest` 0.13.5: async on Tokio, or its `blocking` feature; uses the macOS system proxy by default ([docs](https://docs.rs/reqwest/0.13.5/reqwest/)). Natural choice inside Tauri, which already ships Tokio. |

## Bundle and signing implications

- **A `.app` bundle is required for two features, not just for distribution.** Notifications via `UNUserNotificationCenter` (above) and `SMAppService.mainApp`. A bare `cargo run` binary can show the status item (prototypes did) but cannot use those. A small script or `cargo-bundle` that writes `Info.plist` (with `LSUIElement = true` to hide the dock icon) and runs `codesign -s -` is enough for personal use (**Inference**).
- **Keychain "Always Allow" is tied to the code signature.** Two issue reports describe the same mechanism: an ad-hoc signature is a hash of the build, so every rebuild looks like a different app and the Keychain prompts again ([VoltiusApp/voltius#347](https://github.com/VoltiusApp/voltius/issues/347), [NousResearch/hermes-agent#91115](https://github.com/NousResearch/hermes-agent/issues/91115)). Apple's code signing guide defines identity through the designated requirement ([Code Signing Guide](https://developer.apple.com/library/archive/documentation/Security/Conceptual/CodeSigningGuide/AboutCS/AboutCS.html)). For an app that reads Claude Code's own Keychain item, signing local builds with one stable self-signed certificate avoids a prompt per rebuild (**Inference**). This is a developer-workflow choice, not distribution signing, so it stays inside the map's scope.

## Recommendation

**Default: direct AppKit via `objc2` + `objc2-app-kit`, with `security-framework`, `objc2-user-notifications`, `objc2-service-management`, `notify` and `ureq`, packaged as a minimal ad-hoc-signed `.app`.**

Reasons:

1. It is the only option that renders the required rows natively: a menu row per Profile with two real progress bars, built and run in the prototype. `muda`, Slint's tray and Tauri's tray menus are text only.
2. Lightest by a wide margin: 312 KiB and 12 MB, versus ≈ 53 MB across four processes for Tauri.
3. Native behaviour for free: dark mode, accessibility, menu tracking, `NSPopover` when the Sessions list outgrows a menu.
4. One dependency family (`objc2` framework crates) covers UI, notifications and launch at login, and it is the foundation the other stacks already sit on, so its maintenance risk is shared with all of them.
5. Fits the toolchain (MSRV 1.71) and the macOS-only scope.

Accepted costs: more imperative UI code, `define_class!` for actions and delegates, some `unsafe`, and pre-1.0 framework crates with occasional breaking releases. Keep the AppKit code behind a small UI module so the data side (Profiles, limits, Sessions) stays plain Rust and testable.

**Runner-up: Tauri v2 (`tray-icon` feature) + `tauri-nspanel` (or `tauri-plugin-positioner`) for a popover window, with `tauri-plugin-notification` (`notify-rust`) and `reqwest`.** Choose it if the Unified session view becomes a large searchable list where web UI iteration speed outweighs ≈ 40 MB extra memory and a 3.9 MiB binary. Check that `tauri-plugin-autostart`'s LaunchAgent approach is acceptable, or call `SMAppService` through `objc2` instead.

Not recommended: `tray-icon` + `muda` alone (no bars), eframe (no tray, non-native, MSRV ahead of the local toolchain), stale AppKit wrappers (`cacao`, `menubar`, `tray-item`). Slint is a reasonable middle ground for a cross-platform future, but its tray menu is text only and its licence needs a choice.

## Open questions for the spec

- Menu with custom-view rows versus `NSPopover`: a menu is simplest and most native for a handful of Profiles; a popover scales better for the Sessions list and search. Decide when the Unified session view is specified.
- Whether to sign local builds with a self-signed identity to keep Keychain approvals across rebuilds.
