# ipc-road

What the window's main thread pays when the page talks to it, measured on the
webview stack the window ships and on nothing else (t-20972).

A window's main thread serves three kinds of thing for its webviews that are
not the page's own work: every `invoke` (a `fetch("ipc://localhost/<command>")`
that WebKit hands to `wry … url_scheme_handler::start_task`), every event
(`evaluate_script` of a string the page then runs), and a webview being
born beside the main one. The hang report of 2026-10-01 (t-20972) read as if
the first of these were its cause; this tool prices all three, with wry and
tao at the versions `Cargo.lock` names and nothing patched — `start_task` is
timed from wry's own `tracing` spans.

## Build and run

The crate is its own workspace root and is built from a scratch copy beside the
root `Cargo.lock`, so it neither joins the product's build nor rides its lock.
The thumbnail mode compiles the window's own `artifact_webkit.rs` by relative
path, so the copy keeps the two files' places:

    B=$(mktemp -d)
    mkdir -p "$B/tools" "$B/crates/zerocode-shell/src" "$B/ui"
    cp -R tools/ipc-road "$B/tools/"
    cp crates/zerocode-shell/src/artifact_webkit.rs "$B/crates/zerocode-shell/src/"
    cp ui/shell-boot.js ui/hidden-pane-no-bridge.js "$B/ui/"
    cp Cargo.lock rust-toolchain.toml "$B/tools/ipc-road/"
    (cd "$B/tools/ipc-road" && cargo build --release --offline)
    ROAD="$B/tools/ipc-road/target/release/ipc-road"

    # the scheme road: bodies of 100 B to 64 MB in three encodings, bursts of up
    # to 1000 requests, events of up to 4 MB (a few seconds in all)
    "$ROAD" --out road.json [--quick]

    # a webview born beside the main one (a window outside every screen):
    #   page  calm | busy (its script blocks for 7 s) | heavy (1 MB of markup, a canvas)
    #         | layers (3000 compositing layers moved by script every frame)
    #   place parked (outside the window, as the thumbnail pane is) | hidden | none
    "$ROAD" --out birth.json --birth heavy:parked

    # the artifact gallery's thumbnail pane, step by step as the window runs it
    # (pane born in round 1; `:store` names a data store as the window does,
    # `:window` puts the pane in a window of its own outside every screen
    # instead of parking it in the main window, `:poke` resizes the main
    # window by a pixel every 50 ms meanwhile)
    "$ROAD" --out thumb.json --thumb heavy:store[:window][:poke]

    # `:hold<ms>` keeps the page that long after it finished loading, before the
    # snapshot, and reads the run loop for all of it (the `hold` step): the
    # incident's pane held its page for about seven seconds
    "$ROAD" --out hold.json --thumb ui:store:hold7000 --ui-page "$PWD/ui/index.html" --assert-stall

    # the same run as a gate: exits 4 and names the step when the run loop stood
    # still longer than `thumb::STALL_BOUND_MS` in any step of any round
    "$ROAD" --out thumb.json --thumb heavy:store --assert-stall

    # a pane the backend does not know, showing the window's own page: the
    # pane's `ipc` road refuses every command on the main thread (as Tauri does)
    # and the page boots with eight refused calls nobody catches.
    #   storm     the window's fault reporter as it was (ui/shell-boot.js at
    #             ba6d76230, src/fault_reporter_before.js): a refused report is
    #             an uncaught rejection, which is a fault, which is reported
    #   stormfix  the reporter `ui/shell-boot.js` has now
    # (`storm32` and `stormfix32` leave 32 refused calls uncaught instead of 8;
    # the window's own boot makes 65 to 100 calls in its first second and a half)
    "$ROAD" --out storm.json --thumb storm:store --assert-stall
    "$ROAD" --out stormfix.json --thumb stormfix:store --assert-stall

    # the real thing: the pane shows the window's own `ui/index.html` (named
    # with --ui-page) behind a Tauri-like shim — `window.__TAURI__.core.invoke`
    # is a fetch of `ipc://localhost/<command>` with Tauri's headers, `listen`
    # an invoke of `plugin:event|listen` — and the pane's `ipc` road refuses
    # every request. Point it at two checkouts to compare them.
    "$ROAD" --out ui.json --thumb ui:store --ui-page "$PWD/ui/index.html" --assert-stall
    # and the same page with the script the window gives its hidden panes
    # (ui/hidden-pane-no-bridge.js), which takes the bridge away after the shim:
    "$ROAD" --out uicut.json --thumb uicut:store --ui-page "$PWD/ui/index.html" --assert-stall

macOS only. Measure on AC power with the machine quiet: the numbers are
microseconds, and a build running beside them is the largest term.

## What it reads

| field | meaning |
| --- | --- |
| `post_total_us` | all of `start_task` for one POST: WebKit's request, wry's copy of the body and headers, the handler |
| `post_conversion_us` | `start_task` minus the handler: what happens before the app sees the request |
| `post_handler_us` | the handler: Tauri's JSON parse into a `serde_json::Value`, then the reply handed to another thread |
| `main_thread_ms` | the sum of `start_task` over the phase |
| `loop_gap_ms` | how long the main thread's run loop stood still between two of its turns (it asks to wake every millisecond) |
| `eval_us` | `evaluate_script` of N bytes: what an event of that size costs |
| `birth_call_ms`, `load_finished_ms`, `idle`, `after` | the birth mode: the call that makes the webview, when its page finished, and the run loop's gaps before and after |
| `birth_ms`, `load_ms`, `snapshot_ms`, `worst_gap_ms`, `steps` | the thumbnail mode, per round: the pane's birth, the page's load, the snapshot, and the longest stand-still of the run loop in each step (`worst_gap_ms` is the largest) |
| `requests`, `request_main_ms` | the thumbnail mode, per round: how many custom-scheme requests the main thread served and the time `start_task` took in all of them |
| `most_asked` | the thumbnail mode's refusing pages (`storm*`, `ui`): the commands the pane asked for in the round, most asked first |

`ui/tests/ipc-census.mjs` is the other half: what the page itself sends, counted
in the window harness (`WINDOW_IPC_CENSUS=<file> WINDOW_SUITES=<suite> node
ui/tests/window.mjs`), body bytes and repeats per command.
