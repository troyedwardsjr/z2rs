//! `z2-native` binary: `--headless` CI surface or the windowed frontend.
//!
//! Routing: `--headless` (anywhere in argv) → [`z2_native::headless`];
//! otherwise → [`z2_native::app`] windowed loop. Nothing here opens a window
//! or audio device except via `run_windowed`.

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    if z2_native::headless::is_headless(&argv) {
        std::process::exit(z2_native::headless::run_argv(&argv));
    }
    let args = match z2_native::app::parse_native_args(&argv) {
        Ok(a) => a,
        Err(msg) => {
            // `--help` prints usage as the message with exit 0; unknown flags
            // include the usage text and exit 2.
            if msg.starts_with("usage:") {
                println!("{msg}");
                std::process::exit(0);
            }
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };
    if let Err(e) = z2_native::app::run_windowed(&args) {
        eprintln!("z2-native: {e}");
        std::process::exit(1);
    }
}
