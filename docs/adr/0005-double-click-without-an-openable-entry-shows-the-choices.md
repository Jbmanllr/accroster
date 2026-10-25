# Double-click on a desktop Session without an openable entry shows the choices

Double-click (and a Recent Sessions item) opens a Session in the client it was last used in. For a Session last used in the desktop app, that means its entry in the desktop's Active Profile, through `claude://code/continue`. When that Profile has no usable entry for it (none, or an archived one, which the link can't open), the app opens All Sessions with the Session selected instead of opening it, so its Open section spells out the choices: open it as a new entry in the Profile Claude uses (`claude --desktop --resume`), or switch Claude to a Profile that has its entry, then open it.

## Considered Options

- **Always open it in the desktop app**, as "open a Session in the client it was last used in" reads. Rejected: without an entry the only route is `claude --desktop --resume`, which adds a new entry in the Profile Claude happens to use, without the Session's title or archive state and charged to that Profile. A double-click shouldn't make that choice silently.
- **Switch Claude to a Profile that has the entry.** Rejected as a default: a Guided switch asks the user to sign out and in, too much for a double-click.

## Consequences

- A double-click on such a Session is one more click away from opening it.
- Sessions last used in the CLI, or whose entry the desktop's Active Profile has, open at once as before.
