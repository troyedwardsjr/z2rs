//! Lists gamepads gilrs sees and prints raw events for a few seconds.
//!
//! `cargo run -q -p z2-native --example pad_probe [seconds]`
fn main() {
    // Default filters off: shows raw axis values (the dpad-axis → button
    // conversion otherwise hides what a clone pad actually reports).
    let raw = std::env::args().any(|a| a == "--raw");
    let mut g = z2_native::input::new_gilrs(!raw).expect("gilrs init");
    let secs: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let end = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    #[cfg(target_os = "macos")]
    let mut gc_last = 0u8;
    // This probe never owns key focus; GameController withholds input from
    // unfocused apps unless asked.
    #[cfg(target_os = "macos")]
    unsafe {
        objc2_game_controller::GCController::setShouldMonitorBackgroundEvents(true);
    }
    while std::time::Instant::now() < end {
        // GameController pads (Xbox on macOS): pump the main run loop so
        // connect notifications land, then print the NES byte on change.
        #[cfg(target_os = "macos")]
        {
            use objc2_foundation::{NSDate, NSRunLoop};
            NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.001));
            let b = z2_native::gc_pad::poll();
            if b != gc_last {
                println!(
                    "GameController NES byte {b:08b} (bits: Right Left Down Up Start Select B A)"
                );
                gc_last = b;
            }
        }
        while let Some(ev) = g.next_event() {
            // Idle adapter ports spam tiny axis noise; keep only real motion.
            if let gilrs::EventType::AxisChanged(_, v, _) = ev.event {
                if !raw && v.abs() < 0.5 {
                    continue;
                }
            }
            println!("{:?} {:?} {:?}", ev.id, g.gamepad(ev.id).name(), ev.event);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    #[cfg(target_os = "macos")]
    unsafe {
        use objc2_game_controller::GCDevice;
        for c in objc2_game_controller::GCController::controllers().iter() {
            println!(
                "GameController: vendor={:?} extended={}",
                c.vendorName().map(|n| n.to_string()),
                c.extendedGamepad().is_some()
            );
        }
    }
    for (id, gp) in g.gamepads() {
        println!(
            "pad {id:?}: name={:?} os_name={:?} connected={} mapping={:?} uuid={:x?} vid={:?} pid={:?}",
            gp.name(),
            gp.os_name(),
            gp.is_connected(),
            gp.mapping_source(),
            gp.uuid(),
            gp.vendor_id(),
            gp.product_id()
        );
    }
}
