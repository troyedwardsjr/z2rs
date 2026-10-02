//! Print the preset flag strings, or describe a flag string.
//!
//! ```sh
//! cargo run -p z2-rando --example flags            # presets
//! cargo run -p z2-rando --example flags -- STRING  # what STRING changes
//! cargo run -p z2-rando --example flags -- --json '{"towns":{"shorten_wizards":true}}'
//!                                                  # flag string for JSON options
//! ```

use z2_rando::flags::{Flags, Preset};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        for p in Preset::ALL {
            println!("{:<22} {}", p.label(), p.flags().to_flag_string());
        }
        return;
    }
    if args.len() == 2 && args[0] == "--json" {
        match serde_json::from_str::<Flags>(&args[1]) {
            Ok(f) => println!("{}", f.to_flag_string()),
            Err(e) => {
                eprintln!("--json: {e}");
                std::process::exit(2);
            }
        }
        return;
    }
    for s in args {
        match Flags::from_flag_string(&s) {
            Ok(f) => {
                println!("{s}: {} option(s) changed", f.describe_changes().len());
                for (m, field, v) in f.describe_changes() {
                    println!("  {m}.{field} = {v}");
                }
            }
            Err(e) => {
                eprintln!("{s}: {e}");
                std::process::exit(2);
            }
        }
    }
}
