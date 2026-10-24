# Claude account manager

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

## Usage

**Session limit**:
The rolling 5-hour usage allowance of a Profile.
_Avoid_: hourly limit, session (alone)

**Weekly limit**:
The rolling 7-day usage allowance of a Profile, including any model-specific weekly allowance.
_Avoid_: monthly limit

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
