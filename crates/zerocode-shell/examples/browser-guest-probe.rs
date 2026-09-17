//! Native compatibility probe. No shell state, accounts or orchestration.
//! cargo run -p zerocode-shell --example browser-guest-probe -- URL USER_AGENT [SECONDS] [raw]
//! The normal mode uses the production guest controller; `raw` keeps Tauri's
//! scripts for comparison. Each run uses an ephemeral WebKit data store.

#[cfg(target_os = "macos")]
#[path = "../src/browser_guest_runtime.rs"]
mod guest;

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Duration;

    let mut args = std::env::args().skip(1);
    let url: tauri::Url = args.next().ok_or("URL is required")?.parse()?;
    let user_agent = args.next().ok_or("USER_AGENT is required")?;
    let seconds = args.next().map_or(Ok(140), |value| value.parse::<u64>())?;
    let raw = args.next().as_deref() == Some("raw");
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    context.config_mut().identifier = "dev.zerocode.guest-probe".to_string();
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(move |app| {
            let window = tauri::window::WindowBuilder::new(app, "probe")
                .title(format!("Guest probe — closes automatically after {seconds}s"))
                .inner_size(1100.0, 760.0)
                .build()?;
            let scripts = if raw {
                &[][..]
            } else {
                guest::BROWSER_GUEST_SCRIPTS
            };
            let builder = scripts.iter().fold(
                tauri::webview::WebviewBuilder::new(
                    "guest",
                    tauri::WebviewUrl::External("about:blank".parse()?),
                )
                .incognito(true)
                .user_agent(&user_agent)
                .on_navigation(|url| {
                    println!("navigation origin {}", url.origin().ascii_serialization());
                    true
                }),
                |builder, source| {
                    builder.initialization_script(guest::browser_guest_script(source))
                },
            );
            let pane = window.add_child(
                builder,
                tauri::LogicalPosition::new(0.0, 0.0),
                tauri::LogicalSize::new(1100.0, 760.0),
            )?;
            if !raw {
                guest::prepare_browser_guest(&pane, Duration::from_secs(5))?;
            }
            pane.navigate(url)?;
            pane.set_focus()?;
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let expression = guest::browser_guest_script(include_str!(
                    "../../../ui/tests/browser-guest-probe.js"
                ));
                for elapsed in (0..=seconds).step_by(10) {
                    let script = format!(
                        "(() => {{ const facts = {expression}; \
                         facts.frames = Array.from(document.querySelectorAll('iframe'), f => ({{ \
                         origin: new URL(f.src || 'about:blank', location.href).origin, \
                         title: f.title }})); \
                         facts.responseFields = document.querySelectorAll('[name=cf-turnstile-response]').length; \
                         return JSON.stringify(facts); }})()"
                    );
                    let (answer, mut read) = tokio::sync::mpsc::channel(1);
                    let sent = pane.eval_with_callback(script, move |value| {
                        let _ = answer.try_send(value);
                    });
                    if let Err(error) = sent {
                        eprintln!("eval send failed: {error}");
                    } else {
                        match tokio::time::timeout(Duration::from_secs(5), read.recv()).await {
                            Ok(Some(value)) => println!("{elapsed}s {value}"),
                            result => eprintln!("{elapsed}s eval callback: {result:?}"),
                        }
                    }
                    if elapsed < seconds {
                        tokio::time::sleep(Duration::from_secs(10)).await;
                    }
                }
                println!("probe complete {seconds}s");
                handle.exit(0);
            });
            Ok(())
        })
        .run(context)?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("The native guest compatibility probe requires macOS.");
}
