# Terminals, SSH hosts and remote workspaces: `zerocode-ssh`

Read this when the target is a shell, a saved SSH host, a remote workspace or a remote server. The text was moved here word for word from the former single-file guide.

Where a paragraph below says "above" or "below", it means the whole guide, which is now split over several files: the table in `SKILL.md` says which file holds what.

## Terminals, SSH hosts and remote workspaces

Everything ZeroCode can already reach answers here, and every verb lands in a
pane the user can watch and stop:

```text
zerocode-ssh list --json
zerocode-ssh open --host <listed-id> --json
zerocode-ssh open --local [--cwd <path-inside-the-workspace>] --json
zerocode-ssh send --pane <id> --text 'uname -a' --enter --json
zerocode-ssh read --pane <id>
```

`list` names every saved SSH host, every remote workspace, and every pane this
window holds. Never assume a host name: open the ids `list` reports. `open`
answers with the `pane` id every later verb addresses, so there is no polling
step. `--host` and `--local` reach different machines and exactly one must be
given.

`send` types; it does not submit unless `--enter` is passed, and the text may
not carry control characters — that is what `--enter` is for. Keep a secret out
of process listings with stdin:

```sh
printf '%s' "$SECRET" | zerocode-ssh send --pane <id> --text-stdin --enter --json
```

`read` returns the pane's visible screen as text. A command is not finished
because `send` succeeded; read the pane again until the prompt is back. Do not
open a shell with `zerocode-computer` or drive an existing terminal window
through accessibility — a pane opened here is the same terminal, addressable
and readable.
