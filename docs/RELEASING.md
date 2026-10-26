# Releasing AccRoster

For the maintainer. A release is a notarized `AccRoster-<version>.dmg` on the repository's GitHub Releases page, built by `scripts/release.sh` and published by pushing a `vX.Y.Z` tag. Users never see any of this; their steps are in the README's [Install](../README.md#install).

## One-time setup

Only the owner can do this. You need a Mac, admin rights on the repository and USD 99 a year for Apple. Do the steps in order.

1. **Join the Apple Developer Program as an individual** at [developer.apple.com/programs/enroll](https://developer.apple.com/programs/enroll). You need an Apple Account with two-factor authentication on and your legal name; the fee is paid at enrollment. Apple promises no approval time: if there is no confirmation email within a day, contact Apple Developer Support. After approval, note your **Team ID** (Membership details in your developer account).
2. **Create the Developer ID Application certificate.** Only the Account Holder can, which an individual is.
   1. On your Mac, open Keychain Access → Keychain Access menu → Certificate Assistant → Request a Certificate From a Certificate Authority. Enter your email and name, choose "Saved to disk", and save the `.certSigningRequest`.
   2. In developer.apple.com → Certificates, Identifiers & Profiles → Certificates → **+** → Software → **Developer ID** → **Developer ID Application**, upload the request, download the `.cer` and double-click it to add it to your login keychain.
   3. In Keychain Access → My Certificates, find "Developer ID Application: <your name> (<Team ID>)", right-click → Export → `.p12`, and set a strong password. The certificate's full name is the value of `DEVELOPER_ID_NAME` in step 4.
3. **Create an App Store Connect API key for notarization.** In App Store Connect → Users and Access → Integrations → App Store Connect API → Team Keys → **+**, name it "AccRoster notarization" and give it access **Developer**. Download the `.p8` (Apple lets you download it once) and note the **Key ID** and the **Issuer ID** shown above the list.
4. **Add the GitHub Environment and its secrets.** In the repository, Settings → Environments → New environment `release`. Under Deployment branches and tags choose Selected branches and tags, and add the tag rule `v*`. Optionally add yourself as a required reviewer, so each release waits for your approval. In that environment add:

   | Name | Kind | Value |
   |---|---|---|
   | `DEVELOPER_ID_P12_BASE64` | secret | output of `base64 -i DeveloperID.p12 \| pbcopy` |
   | `DEVELOPER_ID_P12_PASSWORD` | secret | the password you set when exporting the `.p12` |
   | `KEYCHAIN_PASSWORD` | secret | any new random string; it protects the runner's temporary keychain |
   | `NOTARY_KEY_P8_BASE64` | secret | output of `base64 -i AuthKey_XXXX.p8 \| pbcopy` |
   | `NOTARY_KEY_ID` | secret | the Key ID from step 3 |
   | `NOTARY_ISSUER_ID` | secret | the Issuer ID from step 3 |
   | `DEVELOPER_ID_NAME` | variable | the certificate's full name from step 2.3 |

   Only the release job can read these, and only on tags matching `v*`. A pull request never can.
5. **Delete the local `.p12` and `.p8`** once they are in GitHub. Keep the certificate in your keychain if you want to sign locally.
6. **Ship the first release.** Cut it as below, then run the [first-release checklist](#first-release-checklist).

### How the secrets reach the script

The release workflow, `.github/workflows/release.yml`, decodes `DEVELOPER_ID_P12_BASE64` into a temporary keychain on the runner and `NOTARY_KEY_P8_BASE64` into a temporary file, then runs the script with the names the script reads:

| Environment variable | Set from |
|---|---|
| `DEVELOPER_ID_NAME` | the variable `DEVELOPER_ID_NAME` |
| `NOTARY_KEY_P8` | the **path** to the decoded `.p8` file, not its contents or its base64 |
| `NOTARY_KEY_ID` | the secret `NOTARY_KEY_ID` |
| `NOTARY_ISSUER_ID` | the secret `NOTARY_ISSUER_ID` |

The temporary keychain must be on the user keychain search list, because the script looks for the signing identity there. An `always()` cleanup step deletes the keychain and the `.p8` file.

## Cut a release

1. Set `version` under `[workspace.package]` in `Cargo.toml` to the new version, for example `0.2.0`. The app's `CFBundleShortVersionString` comes from it.
2. Merge that change to `main` through a pull request. CI, including the dry run, must pass.
3. Tag the merge commit and push the tag:

   ```sh
   git tag v0.2.0
   git push origin v0.2.0
   ```

4. The Release workflow starts. It fails at once if the tag and `Cargo.toml` disagree, runs the same format, lint and test gate as CI, waits for your approval if you set a required reviewer, runs `scripts/release.sh --developer-id --version v0.2.0`, writes `AccRoster-0.2.0.dmg.sha256` with `shasum -a 256`, and creates the GitHub Release with generated notes, the dmg and the checksum file. The script writes no checksum itself.
5. Open the Releases page and check the dmg is there. For the first release, run the checklist below.

Before it builds anything, the script exits non-zero if the tag and `Cargo.toml` disagree (`version mismatch: releasing X but Cargo.toml says Y`) or if a variable above is missing.

### What the script does

`scripts/release.sh --developer-id --version vX.Y.Z` builds both Mac targets and joins them into one universal app, signs it with the hardened runtime and `bundle/AccRoster.entitlements` (Apple Events only), notarizes and staples the app, builds and signs the dmg, then notarizes and staples the dmg. That is two submissions, so the app opens offline after it is copied out of the dmg. It then checks what a user would get: `codesign --verify`, both architectures with `lipo -archs`, the version, the dmg contents, `spctl` on the app and the dmg, and `stapler validate` on both. Any failure exits non-zero. A rejected notarization prints Apple's `notarytool log`. The last line it prints is the dmg's path, `target/dist/AccRoster-<version>.dmg`.

## Try the release build without secrets

`scripts/release.sh --dry-run [--version X.Y.Z]` does the same build with an ad-hoc signature and no notarization, and runs the checks that need no Apple account. Every pull request and push to `main` runs it as `release-dry-run`, after the `check` job passes. To run it locally:

```sh
brew install librsvg   # once: the icon needs rsvg-convert
scripts/release.sh --dry-run
```

A cold run takes about ten minutes, for two release builds. The Developer ID mode can't run without your certificate and notary key.

## Recover from a failed release

A failed run publishes nothing, because the publish step runs last, so users never see a half release. Read the failing step in the Actions log first. The usual causes:

| Message | Cause and fix |
|---|---|
| `version mismatch` | The tag and `Cargo.toml` differ. Fix whichever is wrong. |
| `--developer-id needs <NAME> set`, `NOTARY_KEY_P8 names no file` | A secret or variable from setup step 4 is missing or misnamed. Fix it in the `release` environment. |
| `no valid code signing identity named …` | `DEVELOPER_ID_NAME` doesn't match the certificate's full name, the certificate expired or was revoked, or the keychain import failed. |
| `notarization of … ended Invalid` (any status but `Accepted`) | Apple rejected the build. The log printed under the message says why. |
| `Gatekeeper rejects …`, `stapler validate` failure | Notarization or stapling didn't take. Re-run once; if it repeats, fix the signing. |

Then retry:

1. **A setup problem** (a secret, the certificate): fix it and re-run the failed run from the Actions page. The tag stays.
2. **A problem in the code:** delete the tag, fix the code, merge, and tag again. Nothing was published, so you can keep the same version.

   ```sh
   git push origin --delete v0.2.0
   git tag -d v0.2.0
   ```

   Then repeat [Cut a release](#cut-a-release) from step 1.
3. **A release published with a defect:** delete it on the Releases page, or edit it to mark it as a pre-release, and ship the fix as the next version. Don't replace a published dmg: its SHA-256 is part of the release.

If the certificate expires or is revoked, repeat setup steps 2 and 4 (`DEVELOPER_ID_P12_BASE64`, `DEVELOPER_ID_P12_PASSWORD`, `DEVELOPER_ID_NAME`). The bundle id `io.github.jbmanllr.accroster` and your Team ID stay the same, which is what keeps users' permissions across releases.

## First-release checklist

By hand, on a Mac or a new user account that has never built AccRoster:

1. Download the dmg in Safari, open it, and check it holds `AccRoster.app` and an Applications link. Compare `shasum -a 256 <dmg>` with the release's checksum.
2. Drag AccRoster to Applications and open it: no Gatekeeper warning.
3. The first-run window offers launch at login, and turning it on registers (Settings shows it on; restart to confirm).
4. A threshold notification appears, and macOS asked for notification permission.
5. Resume in Terminal asks for Automation permission once, the prompt names AccRoster, and the Session resumes.
6. Quit, install the next release over it, and check that permissions are kept and nothing is asked again.
7. On an Intel Mac, if you have one, repeat steps 2 to 5. Otherwise the script's `lipo -archs` check is the proof.

Record the results of the launch-at-login check (from `/Applications` and from `~/Applications`), the Resume in Terminal check, and whether stapling only the dmg lets the copied app open offline on ticket #52. If one changes a decision, record it as a new ADR in `docs/adr/`.
