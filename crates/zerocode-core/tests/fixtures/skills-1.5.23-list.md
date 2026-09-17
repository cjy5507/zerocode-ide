# skills 1.5.23 listing fixture

`skills-1.5.23-list.txt` is the actual combined PTY output captured on
2026-09-05 with:

```sh
DO_NOT_TRACK=1 DISABLE_TELEMETRY=1 npx --yes skills@1.5.23 add /tmp/t-2721-cli-fixture --list
```

This is a read-only listing. The detected-agent preamble is emitted even for
`--list`; the command exits before skill installation. No skill install/update
was executed to create this fixture. ANSI and CRLF bytes are kept intentionally.

The disposable source held two authored skills, `web-design` (description:
`Design pages.`) and `rust-review` (description: `Review Rust code.`). Their
SKILL.md files had `name` and `description` frontmatter and a short fixture body.
The former lived at `plugins/web-tools/skills/web-design`, the latter at
`skills/rust-review`. `.claude-plugin/marketplace.json` was:

```json
{"name":"skills-view-fixture","owner":{"name":"Fixture"},"plugins":[{"name":"web-tools","source":"./plugins/web-tools","skills":["./skills/web-design"]}]}
```

The plugin also held `.claude-plugin/plugin.json` with name `web-tools` and
version `1.0.0`. The explicit `skills` entry makes the CLI emit a plain plugin
heading, followed by `General` for the ungrouped skill. Only indented skill-name
rows belong in the parsed result; guide padding and description indentation
must be kept distinct.

Compatibility sources: [skills formatter](https://github.com/vercel-labs/skills/blob/main/src/add.ts),
[Clack log prefix](https://github.com/bombshell-dev/clack/blob/main/packages/prompts/src/log.ts),
[skills agent catalog](https://github.com/vercel-labs/skills/blob/main/src/agents.ts).
The fixture itself and its test name pin the executed package version.
