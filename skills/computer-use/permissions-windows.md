# Permissions and the Windows differences

Read this when a permission is missing or reads `not-granted`, a helper was rebuilt, or the machine runs Windows. The text was moved here word for word from the former single-file guide.

Where a paragraph below says "above" or "below", it means the whole guide, which is now split over several files: the table in `SKILL.md` says which file holds what.

Use `permissions --id accessibility` or `permissions --id screenshots` to
request that OS permission and open its exact System Settings list. The person
turns on **ZeroCode Computer Use**; if missing, use + to add the returned
`helper_app_path`. Returning to the Computer Use settings automatically checks
again. You can also run `permissions` again or use Settings → Computer Use →
Check Again after granting: this restarts the helper so Screen Recording takes
effect. Observe again before using old element indexes. If the row is already
on but still reports `not-granted`, use `permissions --id <id> --reset`, then
have the person enable the new row and check again. Reset affects only that
permission. `requested_os`/`opened_settings` confirm the request and settings
launch; the permission states say whether access is granted. `identity: adhoc`
means a rebuilt helper may need this reset; `local` retains its certificate
identity across local builds.

On Windows there is no permission to grant: `permissions` reports both as
granted whenever the UI Automation client could start. The same commands,
flags, result schema and error codes apply, with these platform facts:

- Coordinates and window bounds are physical pixels (macOS uses points);
  `screenshot.scale` means the same thing on both — divide screenshot pixels
  by it to get window-local coordinates.
- `bundleId` is the AppUserModelId of a packaged (Store) app and `null` for a
  plain executable; `--app` also accepts the executable name without `.exe`.
- An app running elevated (as administrator) cannot be observed or driven
  from a non-elevated ZeroCode: expect `permission_denied` and say so.
- `paste-text` puts the whole clipboard back afterwards — text, images,
  files, HTML/RTF and every other registered format. When the clipboard
  holds something no other process can copy out (an owner-drawn or private
  format, a metafile with no bitmap twin), the clipboard is left untouched
  and the text is typed instead: `action.path` is `synthetic` with
  `fallbackReason: clipboard_not_preservable`. A `clipboard_restore_failed`
  verification means the clipboard was emptied and a format could not be
  written back — say so.
- `type-text` writes through accessibility when the focused field allows a
  read AND a write, and reports `verified`/`value_mismatch`/`write_unconfirmed`
  on that road; it never types the same text again after such a write. A
  field whose value cannot be read (a password field) is typed
  synthetically and reported with `fallbackReason: value_unreadable`.
- A right-click on an element asks the element for its own context menu
  first (`actionName: ShowContextMenu`); the menu that opens under the
  pointer is the target's own and never reads as a stolen click.
- `--modifiers` accepts `ctrl`/`cmdorctrl`, `alt` and `shift`. The Windows
  key (`cmd`/`meta`/`super`/`win`) is refused as a click modifier because
  releasing it after a click opens the Start menu.
- `hotkey ctrl+a` / `cmdorctrl+a` selects all (through the Text pattern when
  the field has one, Ctrl+A otherwise); `hotkey cmd+a` is Win+A on both
  roads — it opens a system panel, not a selection.
- `scroll` and `drag` move the real pointer to the target (macOS posts to
  the process and leaves the cursor where it was).
- Typed text is literal on both roads. Through accessibility the string is
  written as given. Synthetically, a line break (`\n` or `\r\n`) is sent as
  the carriage-return CHARACTER and a tab as the tab character — never as
  the Enter or Tab KEY — so `type-text` never presses a dialog's default
  button, submits a form or moves focus to another field; a multi-line
  edit gets its line break, a single-line edit keeps its one line. Use
  `press-key return`/`tab` when you mean the key.
- Secondary actions are named by UI Automation pattern (`Invoke`, `Toggle`,
  `Select`, `Expand`, `Collapse`, `ScrollDownByPage`, …) instead of macOS
  action names; `--action` matches either spelling case-insensitively.
- `CmdOrCtrl` is Ctrl; `Cmd`/`Meta`/`Super`/`Win` is the Windows key.
