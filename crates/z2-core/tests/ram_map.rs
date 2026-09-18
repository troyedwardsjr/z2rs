//! RAM accessor tests: synthetic RAM only, no ROM.
//!
//! * Round-trips for every accessor kind (u8 single, u16 big-endian pair,
//!   enemy/projectile slots, indexed spell/item arrays).
//! * Every generated address constant matches `ram-map.toml` (the test
//!   greps the TOML source; when the disassembly submodule provides
//!   `third_party/z2disassembly/ram-map.txt`, the same test greps the
//!   listing for each disassembly label).

use z2_core::ram::*;

// ram-map.toml as shipped at the workspace root. build.rs reads the same
// file, so these assertions tie the generated code to its source.
const RAM_MAP_TOML: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../ram-map.toml"));

fn toml_has(name: &str, addr_hex: &str) {
    assert!(
        RAM_MAP_TOML.contains(&format!("name = \"{name}\"")),
        "ram-map.toml lost entry {name}"
    );
    assert!(
        RAM_MAP_TOML.contains(&format!("addr = {addr_hex}"))
            || RAM_MAP_TOML.contains(&format!("base = {addr_hex}")),
        "ram-map.toml lost address {addr_hex} for {name}"
    );
}

fn check_single(name: &str, actual: u16, expected: u16, addr_hex: &str) {
    assert_eq!(actual, expected, "generated address for {name} is wrong");
    assert!(actual < 0x0800, "{name} escapes NES RAM");
    toml_has(name, addr_hex);
}

#[test]
fn single_addresses_match_ram_map_source() {
    check_single("frame_counter", ADDR_FRAME_COUNTER, 0x0012, "0x0012");
    check_single("link_y", ADDR_LINK_Y, 0x0029, "0x0029");
    check_single("link_page", ADDR_LINK_PAGE, 0x003B, "0x003B");
    check_single("link_x", ADDR_LINK_X, 0x004D, "0x004D");
    check_single("link_x_speed", ADDR_LINK_X_SPEED, 0x0070, "0x0070");
    check_single("link_sprite", ADDR_LINK_SPRITE, 0x0080, "0x0080");
    check_single("link_facing", ADDR_LINK_FACING, 0x009F, "0x009F");
    check_single("input_p1_pressed", ADDR_INPUT_P1_PRESSED, 0x00F5, "0x00F5");
    check_single("input_p2_pressed", ADDR_INPUT_P2_PRESSED, 0x00F6, "0x00F6");
    check_single("input_p1_held", ADDR_INPUT_P1_HELD, 0x00F7, "0x00F7");
    check_single("input_p2_held", ADDR_INPUT_P2_HELD, 0x00F8, "0x00F8");
    check_single("nmi_state", ADDR_NMI_STATE, 0x0100, "0x0100");
    check_single("link_x_sub", ADDR_LINK_X_SUB, 0x03D6, "0x03D6");
    check_single("invuln_stun", ADDR_INVULN_STUN, 0x0500, "0x0500");
    check_single("invuln_blink", ADDR_INVULN_BLINK, 0x0518, "0x0518");
    check_single("rng", ADDR_RNG, 0x051B, "0x051B");
    check_single("menu_state", ADDR_MENU_STATE, 0x0524, "0x0524");
    check_single("scene_index", ADDR_SCENE_INDEX, 0x0561, "0x0561");
    check_single("link_y_speed", ADDR_LINK_Y_SPEED, 0x057D, "0x057D");
    check_single("kills_easy", ADDR_KILLS_EASY, 0x05DF, "0x05DF");
    check_single("kills_hard", ADDR_KILLS_HARD, 0x05E0, "0x05E0");
    check_single("lives", ADDR_LIVES, 0x0700, "0x0700");
    check_single("overworld_index", ADDR_OVERWORLD_INDEX, 0x0706, "0x0706");
    check_single("world", ADDR_WORLD, 0x0707, "0x0707");
    check_single("game_mode", ADDR_GAME_MODE, 0x0736, "0x0736");
    check_single("area_index", ADDR_AREA_INDEX, 0x0748, "0x0748");
    check_single("encounter_type", ADDR_ENCOUNTER_TYPE, 0x075A, "0x075A");
    check_single("game_state", ADDR_GAME_STATE, 0x076C, "0x076C");
    check_single("exp_next", ADDR_EXP_NEXT, 0x0770, "0x0770");
    check_single("link_mp", ADDR_LINK_MP, 0x0773, "0x0773");
    check_single("link_hp", ADDR_LINK_HP, 0x0774, "0x0774");
    check_single("exp", ADDR_EXP, 0x0775, "0x0775");
    check_single("attack_level", ADDR_ATTACK_LEVEL, 0x0777, "0x0777");
    check_single("magic_level", ADDR_MAGIC_LEVEL, 0x0778, "0x0778");
    check_single("life_level", ADDR_LIFE_LEVEL, 0x0779, "0x0779");
    check_single("magic_containers", ADDR_MAGIC_CONTAINERS, 0x0783, "0x0783");
    check_single("heart_containers", ADDR_HEART_CONTAINERS, 0x0784, "0x0784");
    check_single("keys", ADDR_KEYS, 0x0793, "0x0793");
    check_single("crystals", ADDR_CRYSTALS, 0x0794, "0x0794");
    check_single("thrust_flags", ADDR_THRUST_FLAGS, 0x0796, "0x0796");
    check_single("deaths", ADDR_DEATHS, 0x079F, "0x079F");
    check_single("projectile_flag", ADDR_PROJECTILE_FLAG, 0x008D, "0x008D");
}

#[test]
fn array_bases_match_ram_map_source() {
    assert_eq!(ENEMY_SLOT_COUNT, 6);
    assert_eq!(PROJECTILE_SLOT_COUNT, 6);
    assert_eq!(SPELL_LEN, 8);
    assert_eq!(ITEM_LEN, 8);
    let bases: &[(&str, u16, &str)] = &[
        ("enemy_y", BASE_ENEMY_Y, "0x002A"),
        ("enemy_page", BASE_ENEMY_PAGE, "0x003C"),
        ("enemy_x", BASE_ENEMY_X, "0x004E"),
        ("enemy_facing", BASE_ENEMY_FACING, "0x0060"),
        ("enemy_speed", BASE_ENEMY_SPEED, "0x0071"),
        ("enemy_id", BASE_ENEMY_ID, "0x00A1"),
        ("enemy_exists", BASE_ENEMY_EXISTS, "0x00B6"),
        ("enemy_hp", BASE_ENEMY_HP, "0x00C2"),
        ("enemy_stun", BASE_ENEMY_STUN, "0x040E"),
        ("projectile_y", BASE_PROJECTILE_Y, "0x0030"),
        ("projectile_page", BASE_PROJECTILE_PAGE, "0x0042"),
        ("projectile_x", BASE_PROJECTILE_X, "0x0054"),
        ("projectile_facing", BASE_PROJECTILE_FACING, "0x0066"),
        ("projectile_speed", BASE_PROJECTILE_SPEED, "0x0077"),
        ("projectile_id", BASE_PROJECTILE_ID, "0x0087"),
        ("spell", BASE_SPELL, "0x077B"),
        ("item", BASE_ITEM, "0x0785"),
    ];
    let expected: &[u16] = &[
        0x002A, 0x003C, 0x004E, 0x0060, 0x0071, 0x00A1, 0x00B6, 0x00C2, 0x040E, 0x0030, 0x0042,
        0x0054, 0x0066, 0x0077, 0x0087, 0x077B, 0x0785,
    ];
    for (i, (name, actual, hex)) in bases.iter().enumerate() {
        assert_eq!(*actual, expected[i], "generated base for {name} is wrong");
        toml_has(name, hex);
    }
    // The curated map must keep covering the whole planned scope. (Count
    // line-anchored headers: the file's own schema comment mentions the
    // header names in prose.)
    let mut entries = 0;
    let mut arrays = 0;
    let mut verified = 0;
    for line in RAM_MAP_TOML.lines() {
        match line.trim() {
            "[[entry]]" => entries += 1,
            "[[array]]" => arrays += 1,
            _ => {}
        }
        if line.trim_start().starts_with("verified = ") {
            verified += 1;
        }
    }
    assert!(entries >= 30, "ram-map.toml lost entries ({entries})");
    assert!(arrays >= 15, "ram-map.toml lost arrays ({arrays})");
    assert_eq!(
        verified,
        entries + arrays,
        "every map block must carry a verified-against-label flag"
    );
}

#[test]
fn u8_single_round_trips() {
    let mut ram = Ram::new();
    ram.set_frame_counter(0xAB);
    ram.set_link_x(0x12);
    ram.set_link_y(0x34);
    ram.set_link_page(0x02);
    ram.set_link_hp(0x08);
    ram.set_link_mp(0x07);
    ram.set_world(0x03);
    ram.set_game_mode(0x06);
    ram.set_game_state(0x01);
    ram.set_rng(0x5A);
    ram.set_input_p1_held(0x80);
    assert_eq!(ram.frame_counter(), 0xAB);
    assert_eq!(ram.link_x(), 0x12);
    assert_eq!(ram.link_y(), 0x34);
    assert_eq!(ram.link_page(), 0x02);
    assert_eq!(ram.link_hp(), 0x08);
    assert_eq!(ram.link_mp(), 0x07);
    assert_eq!(ram.world(), 0x03);
    assert_eq!(ram.game_mode(), 0x06);
    assert_eq!(ram.game_state(), 0x01);
    assert_eq!(ram.rng(), 0x5A);
    assert_eq!(ram.input_p1_held(), 0x80);
    // Raw bytes land at the documented addresses.
    assert_eq!(ram.as_slice()[0x0774], 0x08);
    assert_eq!(ram.as_slice()[0x0012], 0xAB);
}

#[test]
fn u16_big_endian_pairs_round_trip() {
    let mut ram = Ram::new();
    ram.set_exp(0x1234);
    ram.set_exp_next(0x00FF);
    assert_eq!(ram.exp(), 0x1234);
    assert_eq!(ram.exp_next(), 0x00FF);
    // $0775 is the MSB: matches Data Crystal + Mesen memory view order.
    assert_eq!(ram.as_slice()[0x0775], 0x12);
    assert_eq!(ram.as_slice()[0x0776], 0x34);
    assert_eq!(ram.as_slice()[0x0770], 0x00);
    assert_eq!(ram.as_slice()[0x0771], 0xFF);
}

#[test]
fn enemy_slots_round_trip_all_fields() {
    let mut ram = Ram::new();
    for slot in 0..ENEMY_SLOT_COUNT {
        let s = slot as u8;
        let mut e = ram.enemy_mut(slot);
        e.set_y(10 + s);
        e.set_x(20 + s);
        e.set_page(s);
        e.set_facing(1 + (s & 1));
        e.set_speed(30 + s);
        e.set_id(40 + s);
        e.set_exists(1);
        e.set_hp(50 + s);
        e.set_stun(60 + s);
    }
    for slot in 0..ENEMY_SLOT_COUNT {
        let s = slot as u8;
        let e = ram.enemy(slot);
        assert_eq!(e.slot(), slot);
        assert_eq!(e.y(), 10 + s, "slot {slot} y");
        assert_eq!(e.x(), 20 + s, "slot {slot} x");
        assert_eq!(e.page(), s, "slot {slot} page");
        assert_eq!(e.facing(), 1 + (s & 1), "slot {slot} facing");
        assert_eq!(e.speed(), 30 + s, "slot {slot} speed");
        assert_eq!(e.id(), 40 + s, "slot {slot} id");
        assert_eq!(e.exists(), 1, "slot {slot} exists");
        assert_eq!(e.hp(), 50 + s, "slot {slot} hp");
        assert_eq!(e.stun(), 60 + s, "slot {slot} stun");
        // Spot-check raw addressing: slot s lives at base + s.
        assert_eq!(ram.as_slice()[0x00C2 + slot], 50 + s);
        assert_eq!(ram.as_slice()[0x002A + slot], 10 + s);
    }
}

#[test]
fn projectile_slots_round_trip_all_fields() {
    let mut ram = Ram::new();
    for slot in 0..PROJECTILE_SLOT_COUNT {
        let s = slot as u8;
        let mut p = ram.projectile_mut(slot);
        p.set_y(1 + s);
        p.set_page(s);
        p.set_x(2 + s);
        p.set_facing(s & 1);
        p.set_speed(3 + s);
        p.set_id(4 + s);
    }
    for slot in 0..PROJECTILE_SLOT_COUNT {
        let s = slot as u8;
        let p = ram.projectile(slot);
        assert_eq!(p.y(), 1 + s);
        assert_eq!(p.page(), s);
        assert_eq!(p.x(), 2 + s);
        assert_eq!(p.facing(), s & 1);
        assert_eq!(p.speed(), 3 + s);
        assert_eq!(p.id(), 4 + s);
        assert_eq!(ram.as_slice()[0x0030 + slot], 1 + s);
        assert_eq!(ram.as_slice()[0x0087 + slot], 4 + s);
    }
}

#[test]
fn spell_and_item_arrays_round_trip() {
    let mut ram = Ram::new();
    for i in 0..SPELL_LEN {
        ram.set_spell(i, (i as u8) + 1);
    }
    for i in 0..ITEM_LEN {
        ram.set_item(i, if i % 2 == 0 { 1 } else { 0 });
    }
    for i in 0..SPELL_LEN {
        assert_eq!(ram.spell(i), (i as u8) + 1);
    }
    assert_eq!(ram.as_slice()[0x077B], 1); // shield
    assert_eq!(ram.as_slice()[0x0782], 8); // thunder
    assert_eq!(ram.as_slice()[0x0785], 1); // candle
    assert_eq!(ram.as_slice()[0x078C], 0); // magic key
}

#[test]
fn ram_mirror_and_image_helpers() {
    let mut ram = Ram::new();
    ram.write(0x0000, 0x77);
    // NES RAM mirrors every 2 KiB through $1FFF.
    assert_eq!(ram.read(0x0800), 0x77);
    assert_eq!(ram.read(0x1800), 0x77);
    let bytes = *ram.as_slice();
    assert_eq!(bytes.len(), Ram::LEN);
    let back = Ram::from_slice(&bytes).expect("full image round-trips");
    assert_eq!(back, ram);
    assert!(Ram::from_slice(&bytes[..100]).is_err());
    assert!(Ram::from_slice(&[]).is_err());
}

#[test]
#[should_panic(expected = "out of range")]
fn enemy_slot_out_of_range_panics() {
    let ram = Ram::new();
    let _ = ram.enemy(ENEMY_SLOT_COUNT);
}

#[test]
#[should_panic(expected = "out of range")]
fn spell_index_out_of_range_panics() {
    let ram = Ram::new();
    let _ = ram.spell(SPELL_LEN);
}

/// Acceptance: every accessor's address matches the disassembly label it
/// cites — a test greps the listing.
///
/// This parses `ram-map.toml` for `(address, label, verified)` triples and,
/// for every block with `verified = true`, asserts the cited label appears
/// (modulo case/punctuation) on a listing line that mentions that address.
/// The listing is `third_party/z2disassembly/ram-map.txt` (`ADDR ; comment`,
/// `,X`/`,Y` suffixes for indexed tables) plus the formal ca65 symbols in
/// `src/variables.asm` (`joy1_pressed = $F5`, ...). Blocks with
/// `verified = false` are explicitly Data Crystal/note-sourced and are
/// covered by the TOML-structure assertions above instead.
#[test]
fn accessor_labels_match_disassembly_listing_when_present() {
    #[derive(Debug)]
    struct Cited {
        name: String,
        addr: u32,
        label: String,
        verified: bool,
    }
    // Minimal hand parser for our strict schema (mirrors build.rs).
    let mut cited: Vec<Cited> = Vec::new();
    let mut name = String::new();
    let mut addr: Option<u32> = None;
    let mut label = String::new();
    let mut verified = false;
    let mut in_block = false;
    for line in RAM_MAP_TOML.lines() {
        let t = line.trim();
        if t == "[[entry]]" || t == "[[array]]" {
            if in_block {
                cited.push(Cited {
                    name: std::mem::take(&mut name),
                    addr: addr.take().expect("map block needs addr/base"),
                    label: std::mem::take(&mut label),
                    verified,
                });
                verified = false;
            }
            in_block = true;
            continue;
        }
        if !in_block || t.is_empty() || t.starts_with('#') {
            continue;
        }
        let eq = t.find('=').expect("map pair");
        let (k, v) = (t[..eq].trim(), t[eq + 1..].trim());
        match k {
            "name" => name = v.trim_matches('"').to_string(),
            "addr" | "base" => {
                let h = v.strip_prefix("0x").expect("hex addr");
                addr = Some(u32::from_str_radix(h, 16).expect("hex addr"));
            }
            "label" => label = v.trim_matches('"').to_string(),
            "verified" => verified = v == "true",
            _ => {}
        }
    }
    if in_block {
        cited.push(Cited {
            name,
            addr: addr.expect("map block needs addr/base"),
            label,
            verified,
        });
    }
    assert!(!cited.is_empty(), "parsed no blocks from ram-map.toml");

    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let listing_path = root.join("third_party/z2disassembly/ram-map.txt");
    let symbols_path = root.join("third_party/z2disassembly/src/variables.asm");
    if !listing_path.is_file() {
        eprintln!("SKIP label-vs-listing grep: no z2disassembly listing yet; ram-map.toml assertions above still hold");
        return;
    }
    let mut listing = std::fs::read_to_string(&listing_path).expect("read listing");
    if symbols_path.is_file() {
        listing.push('\n');
        listing.push_str(&std::fs::read_to_string(&symbols_path).expect("read variables.asm"));
    }

    fn norm(s: &str) -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_lowercase()
    }
    // Index listing lines by every address they mention: a leading `ADDR`
    // token (ram-map.txt style, incl. `$`-prefixed and `,X`-suffixed) and
    // any `$ADDR` occurrence (variables.asm style).
    let mut by_addr: std::collections::BTreeMap<u32, Vec<String>> =
        std::collections::BTreeMap::new();
    for line in listing.lines() {
        let mut addrs = Vec::new();
        let head = line.split(';').next().unwrap_or("").trim();
        let tok = head.split_whitespace().next().unwrap_or("");
        let tok = tok.strip_prefix('$').unwrap_or(tok);
        let tok = tok.split(',').next().unwrap_or("");
        if !tok.is_empty() && tok.chars().all(|c| c.is_ascii_hexdigit()) {
            if let Ok(a) = u32::from_str_radix(tok, 16) {
                addrs.push(a);
            }
        }
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'$' {
                let mut j = i + 1;
                while j < bytes.len() && (bytes[j] as char).is_ascii_hexdigit() {
                    j += 1;
                }
                if j > i + 1 && j - (i + 1) <= 4 {
                    if let Ok(a) = u32::from_str_radix(&line[i + 1..j], 16) {
                        addrs.push(a);
                    }
                }
                i = j;
            } else {
                i += 1;
            }
        }
        let n = norm(line);
        for a in addrs {
            by_addr.entry(a).or_default().push(n.clone());
        }
    }

    let mut failures = Vec::new();
    let mut checked = 0;
    for c in cited.iter().filter(|c| c.verified) {
        checked += 1;
        let want = norm(&c.label);
        let hit = by_addr
            .get(&c.addr)
            .map(|lines| lines.iter().any(|l| l.contains(&want)))
            .unwrap_or(false);
        if !hit {
            failures.push(format!(
                "{} (${:04X}): label {:?} not found at that address in the listing",
                c.name, c.addr, c.label
            ));
        }
    }
    assert!(
        checked > 0,
        "no verified blocks to check — map regressed to all-unverified"
    );
    assert!(
        failures.is_empty(),
        "listing mismatches:\n{}",
        failures.join("\n")
    );
}
