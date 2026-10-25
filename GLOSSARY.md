# AccRoster

A macOS menu bar app that gathers every Claude account used on one Mac: their usage limits, their sessions, and switching between them for the desktop app and the CLI.

## Accounts

**Login**:
One claude.ai identity (an account id), signed in with an email.
_Avoid_: user, account (when the organization matters)

**Organization**:
The billing and limits holder a Login acts within; a personal plan is a one-person Organization.
_Avoid_: org, team, workspace

**Profile**:
One Login acting within one Organization: the unit the app lists, measures and switches to.
_Avoid_: account, identity

**Active Profile**:
The Profile a given client (the desktop app or the CLI) is currently signed in as; the two clients can differ.
_Avoid_: current account

**Discovered Profile**:
A Profile the app has evidence of on the Mac but holds no sign-in for; listed greyed until signed in.
_Avoid_: found profile, ghost profile, unsigned profile

**Guided switch**:
Changing the desktop app's Active Profile by the user signing out and in themselves, led and then confirmed by the app.
_Avoid_: desktop switch (alone), auto-switch

## Usage

**Session limit**:
The rolling 5-hour usage allowance of a Profile.
_Avoid_: hourly limit, session (alone)

**Weekly limit**:
The rolling 7-day usage allowance of a Profile, including any model-specific weekly allowance.
_Avoid_: monthly limit

**Banked reset**:
A saved, not-yet-used reset of one of a Profile's limits, redeemable until its expiry date.
_Avoid_: reset grant, reset credit

**Session refill**:
A recurring weekly allowance that refills a Profile's Session limit at the cost of its Weekly limit; it never expires and is not a Banked reset.
_Avoid_: weekly reset, session reset, refill (alone)

**Limit reading**:
All of a Profile's limits as fetched at one moment, with the time it was taken.
_Avoid_: usage data, snapshot, figures

## Sessions

**Session**:
One Claude Code conversation, identified by its transcript id, whichever client or Profile created it.
_Avoid_: chat, conversation, thread

**Transcript**:
The on-disk record of a Session's turns; shared by every Profile on the Mac.
_Avoid_: history, log

**Sidebar entry**:
The desktop app's per-Profile record that makes a Session appear in that Profile's Code sidebar, with its title and archive state.
_Avoid_: session file, index entry

**Unified session view**:
The app's own list of every Session on the Mac, across Profiles, from which a Session opens in the CLI or the desktop app.
_Avoid_: session browser, merged sidebar

**Sidebar sync**:
Making a Session's Sidebar entry exist in every Profile's sidebar, so the desktop app shows all Sessions whichever Profile is active.
_Avoid_: session sync (ambiguous), merge

## Safety

**Safety pause**:
A feature that writes Claude's files, stopped by the app itself because Claude changed something the feature relies on; only the user resumes it.
_Avoid_: error, disabled, off (those suggest the user turned it off)
