use gpui::{
    App, Bounds, Context, SharedString, Window, WindowBounds, WindowOptions, div, prelude::*, px,
    rgb, size,
};
use std::{
    env,
    fs::File,
    io::{self, Read},
};
use zerocode_pty::Terminal;

struct Screen(SharedString);

impl Render for Screen {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0xffffff))
            .text_color(rgb(0))
            .font_family("monospace")
            .child(self.0.clone())
    }
}

fn main() -> io::Result<()> {
    let capture = env::var_os("GPUI_PROBE_IMAGE");
    if capture.is_some() && !cfg!(feature = "capture") {
        return Err(io::Error::other(
            "GPUI_PROBE_IMAGE requires --features capture",
        ));
    }
    let mut terminal = Terminal::new(24, 80);
    if let Some(path) = env::args_os().nth(1) {
        let mut file = File::open(path)?;
        let mut chunk = [0; 8192];
        loop {
            let count = file.read(&mut chunk)?;
            if count == 0 {
                break;
            }
            terminal.feed(&chunk[..count]);
        }
    } else {
        terminal.feed("Rust PTY → GPUI\r\n한글 · e\u{301} · 😀\r\n".as_bytes());
    }
    let text = SharedString::from(terminal.grid().visible_text());
    gpui_platform::application().run(move |cx: &mut App| {
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(640.), px(240.)), cx);
        let options = WindowOptions {
            show: capture.is_none(),
            focus: capture.is_none(),
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };
        let _window = cx
            .open_window(options, |_, cx| cx.new(|_| Screen(text)))
            .expect("open the GPUI smoke-test window");
        #[cfg(feature = "capture")]
        if let Some(path) = capture {
            _window
                .update(cx, |_, window, _| {
                    window
                        .render_to_image()
                        .expect("render the probe scene")
                        .save(path)
                        .expect("write the probe image");
                })
                .expect("read the probe window");
            cx.quit();
        }
    });
    Ok(())
}
