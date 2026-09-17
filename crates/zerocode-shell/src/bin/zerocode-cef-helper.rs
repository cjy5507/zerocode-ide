#[cfg(target_os = "macos")]
fn main() {
    use cef::{api_hash, args::Args, execute_process, library_loader, sys};

    let args = Args::new();
    let mut sandbox = cef::sandbox::Sandbox::new();
    sandbox.initialize(args.as_main_args());

    let loader = library_loader::LibraryLoader::new(
        &std::env::current_exe().expect("CEF helper executable path"),
        true,
    );
    assert!(loader.load(), "failed to load the bundled CEF framework");
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    let exit_code = execute_process(
        Some(args.as_main_args()),
        None::<&mut cef::App>,
        std::ptr::null_mut(),
    );
    drop(loader);
    drop(sandbox);
    std::process::exit(exit_code);
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("zerocode-cef-helper is only used by the macOS Chromium browser bundle");
}
