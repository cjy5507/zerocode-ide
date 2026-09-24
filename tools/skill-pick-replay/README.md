# Skill suggestion replay

`seed.py --out /tmp/skill-turns.jsonl` reads the last seven days of zo
transcripts. Its output contains SHA-256 prefixes, first-load hashes, and
counts; it contains no request text, skill names, paths, or session IDs.

For an accuracy comparison, independently label each request with
`{"id":"…","skill":"<hashed skill name>"}` or `"skill":null` when no installed
skill covers it. Then run the agent on the same requests with a suggestion
and record its first load in `{"id":"…","firstLoad":"…","inputTokens":123,
"requests":2}` (null for no load). Pass these files as `--gold` and
`--assisted`. A suggestion alone is not the agent's actual load and cannot
replace the assisted run. The report gives both error rates, a Wilson lower
bound for covered-request accuracy, fixed and harmed counts, and billed Jev
input tokens at the documented $0.042 per million tokens.

The seed intentionally does not infer gold labels from historical Skill
calls: an agent's old choice may be exactly the mistake under test.

It also counts the live seat's separate first-load agreement and unused-load
proxy labels when a `skill-search.jsonl` exists. The proxy means a loaded
skill had no later non-skill tool call or substantial quote from its returned
body in that turn; it is an observation, not a verified usefulness label.
