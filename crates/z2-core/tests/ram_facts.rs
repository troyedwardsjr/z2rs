//! GameFacts tests: synthetic RAM only, no ROM.
//!
//! * `facts()` exposes exactly the bytes written to RAM (JSON snapshot).
//! * 10-field Mesen spot-check mapping (see `mesen_spot_check_mapping`):
//!   `Ram` IS the NES CPU-RAM image, so a reviewer pauses the same state
//!   in Mesen's Debugger → Memory Viewer, reads the listed address, and
//!   compares with the `facts()` JSON value documented here.
//! * `facts_diff` reports exactly the changed leaves.

use serde_json::Value;
use z2_core::facts::{facts_diff, Game};
use z2_core::ram::Ram;

/// Build the canonical synthetic state shared by the snapshot tests.
fn synthetic_ram() -> Ram {
    let mut ram = Ram::new();
    // Link.
    ram.set_link_x(0x40);
    ram.set_link_y(0x80);
    ram.set_link_page(0x01);
    ram.set_link_x_speed(0x02);
    ram.set_link_y_speed(0xFE);
    ram.set_link_facing(0x01);
    ram.set_link_hp(0x06);
    ram.set_link_mp(0x05);
    ram.set_exp(0x012C);
    ram.set_attack_level(0x03);
    ram.set_magic_level(0x02);
    ram.set_life_level(0x04);
    ram.set_lives(0x03);
    ram.set_keys(0x02);
    // Spells: shield + fire learned.
    ram.set_spell(0, 1);
    ram.set_spell(4, 1);
    // Items: candle + hammer owned.
    ram.set_item(0, 1);
    ram.set_item(6, 1);
    // World / mode.
    ram.set_overworld_index(0x02);
    ram.set_world(0x04);
    ram.set_scene_index(0x24);
    ram.set_area_index(0x07);
    ram.set_encounter_type(0x02);
    ram.set_crystals(0x05);
    ram.set_game_mode(0x01);
    ram.set_game_state(0x01);
    ram.set_menu_state(0x00);
    // Enemy slot 0: a live foe; slot 5: empty.
    ram.enemy_mut(0).set_id(0x0A);
    ram.enemy_mut(0).set_exists(0x01);
    ram.enemy_mut(0).set_x(0x50);
    ram.enemy_mut(0).set_y(0x60);
    ram.enemy_mut(0).set_page(0x00);
    ram.enemy_mut(0).set_facing(0x01);
    ram.enemy_mut(0).set_speed(0x03);
    ram.enemy_mut(0).set_hp(0x07);
    ram.enemy_mut(0).set_stun(0x00);
    // Projectile slot 1: active fireball-ish.
    ram.projectile_mut(1).set_id(0x02);
    ram.projectile_mut(1).set_x(0x44);
    ram.projectile_mut(1).set_y(0x70);
    ram.projectile_mut(1).set_page(0x01);
    ram.projectile_mut(1).set_facing(0x00);
    ram.projectile_mut(1).set_speed(0x04);
    ram.set_projectile_flag(0x01);
    // Timers / rng / input.
    ram.set_frame_counter(0x9C);
    ram.set_invuln_stun(0x00);
    ram.set_invuln_blink(0x02);
    ram.set_kills_easy(0x03);
    ram.set_kills_hard(0x01);
    ram.set_rng(0xA5);
    ram.set_input_p1_pressed(0x80);
    ram.set_input_p2_pressed(0x00);
    ram.set_input_p1_held(0x81);
    ram.set_input_p2_held(0x00);
    ram
}

fn facts_json(ram: &Ram) -> Value {
    serde_json::to_value(Game::new(ram.clone()).facts()).expect("facts serialize")
}

/// Acceptance aid: 10 fields a reviewer can spot-check in Mesen's memory
/// viewer against `facts()` JSON for the same state.
///
/// | # | facts JSON path      | CPU-RAM | Mesen: Debugger → Memory Viewer |
/// |---|----------------------|---------|---------------------------------|
/// | 1 | link.hp              | $0774   | byte at 0774                    |
/// | 2 | link.mp              | $0773   | byte at 0773                    |
/// | 3 | link.x               | $004D   | byte at 004D                    |
/// | 4 | link.y               | $0029   | byte at 0029                    |
/// | 5 | link.exp             | $0775-6 | big-endian pair 0775 (MSB) 0776 |
/// | 6 | world.world          | $0707   | byte at 0707                    |
/// | 7 | mode.mode            | $0736   | byte at 0736                    |
/// | 8 | enemies[0].hp        | $00C2   | byte at 00C2 (slot0 = base+0)   |
/// | 9 | rng                  | $051B   | byte at 051B                    |
/// |10 | input.p1_held        | $00F7   | byte at 00F7                    |
#[test]
fn mesen_spot_check_mapping() {
    let v = facts_json(&synthetic_ram());
    let get = |ptr: &str| {
        v.pointer(ptr)
            .unwrap_or_else(|| panic!("facts JSON lacks {ptr}"))
    };
    assert_eq!(get("/link/hp"), &Value::from(0x06u8)); // $0774
    assert_eq!(get("/link/mp"), &Value::from(0x05u8)); // $0773
    assert_eq!(get("/link/x"), &Value::from(0x40u8)); // $004D
    assert_eq!(get("/link/y"), &Value::from(0x80u8)); // $0029
    assert_eq!(get("/link/exp"), &Value::from(0x012Cu16)); // $0775-76 BE
    assert_eq!(get("/world/world"), &Value::from(0x04u8)); // $0707
    assert_eq!(get("/mode/mode"), &Value::from(0x01u8)); // $0736
    assert_eq!(get("/enemies/0/hp"), &Value::from(0x07u8)); // $00C2
    assert_eq!(get("/rng"), &Value::from(0xA5u8)); // $051B
    assert_eq!(get("/input/p1_held"), &Value::from(0x81u8)); // $00F7
}

#[test]
fn facts_json_snapshot_full_state() {
    let v = facts_json(&synthetic_ram());
    // Link block.
    assert_eq!(v.pointer("/link/x_speed").unwrap(), &Value::from(0x02u8));
    assert_eq!(v.pointer("/link/y_speed").unwrap(), &Value::from(0xFEu8));
    assert_eq!(v.pointer("/link/facing").unwrap(), &Value::from(0x01u8));
    assert_eq!(v.pointer("/link/attack").unwrap(), &Value::from(0x03u8));
    assert_eq!(v.pointer("/link/magic").unwrap(), &Value::from(0x02u8));
    assert_eq!(v.pointer("/link/life").unwrap(), &Value::from(0x04u8));
    assert_eq!(v.pointer("/link/lives").unwrap(), &Value::from(0x03u8));
    assert_eq!(v.pointer("/link/keys").unwrap(), &Value::from(0x02u8));
    assert_eq!(v.pointer("/link/page").unwrap(), &Value::from(0x01u8));
    // Spells / items decode nonzero bytes to true.
    assert_eq!(v.pointer("/spells/shield").unwrap(), &Value::from(true));
    assert_eq!(v.pointer("/spells/fire").unwrap(), &Value::from(true));
    assert_eq!(v.pointer("/spells/thunder").unwrap(), &Value::from(false));
    assert_eq!(v.pointer("/items/candle").unwrap(), &Value::from(true));
    assert_eq!(v.pointer("/items/hammer").unwrap(), &Value::from(true));
    assert_eq!(v.pointer("/items/raft").unwrap(), &Value::from(false));
    // World / mode.
    assert_eq!(v.pointer("/world/overworld").unwrap(), &Value::from(0x02u8));
    assert_eq!(v.pointer("/world/scene").unwrap(), &Value::from(0x24u8));
    assert_eq!(v.pointer("/world/area").unwrap(), &Value::from(0x07u8));
    assert_eq!(v.pointer("/world/encounter").unwrap(), &Value::from(0x02u8));
    assert_eq!(v.pointer("/world/crystals").unwrap(), &Value::from(0x05u8));
    assert_eq!(v.pointer("/mode/state").unwrap(), &Value::from(0x01u8));
    assert_eq!(v.pointer("/mode/menu").unwrap(), &Value::from(0x00u8));
    // Enemy slot 0 populated; untouched slots read zero.
    let e0 = v.pointer("/enemies/0").unwrap();
    assert_eq!(e0.get("slot").unwrap(), &Value::from(0u8));
    assert_eq!(e0.get("id").unwrap(), &Value::from(0x0Au8));
    assert_eq!(e0.get("exists").unwrap(), &Value::from(0x01u8));
    assert_eq!(e0.get("x").unwrap(), &Value::from(0x50u8));
    assert_eq!(v.pointer("/enemies/5/hp").unwrap(), &Value::from(0u8));
    assert_eq!(
        v.as_object().unwrap()["enemies"].as_array().unwrap().len(),
        6
    );
    // Projectile slot 1 populated.
    let p1 = v.pointer("/projectiles/1").unwrap();
    assert_eq!(p1.get("id").unwrap(), &Value::from(0x02u8));
    assert_eq!(p1.get("x").unwrap(), &Value::from(0x44u8));
    assert_eq!(v.pointer("/projectile_flag").unwrap(), &Value::from(0x01u8));
    assert_eq!(
        v.as_object().unwrap()["projectiles"]
            .as_array()
            .unwrap()
            .len(),
        6
    );
    // Timers / input.
    assert_eq!(v.pointer("/timers/frame").unwrap(), &Value::from(0x9Cu8));
    assert_eq!(
        v.pointer("/timers/invuln_blink").unwrap(),
        &Value::from(0x02u8)
    );
    assert_eq!(
        v.pointer("/timers/kills_easy").unwrap(),
        &Value::from(0x03u8)
    );
    assert_eq!(
        v.pointer("/timers/kills_hard").unwrap(),
        &Value::from(0x01u8)
    );
    assert_eq!(
        v.pointer("/input/p1_pressed").unwrap(),
        &Value::from(0x80u8)
    );
    // Full pretty snapshot: byte-identical contract for native vs wasm.
    let pretty = Game::new(synthetic_ram()).facts().to_json_pretty();
    assert_eq!(
        pretty, EXPECTED_PRETTY_SNAPSHOT,
        "GameFacts JSON drifted; update Mesen table + consumers"
    );
}

// Canonical pretty JSON of `synthetic_ram()`. Generated once from verified
// output — any change here must be explainable as a RAM-map change.
const EXPECTED_PRETTY_SNAPSHOT: &str = r#"{
  "link": {
    "x": 64,
    "y": 128,
    "page": 1,
    "x_speed": 2,
    "y_speed": 254,
    "facing": 1,
    "hp": 6,
    "mp": 5,
    "exp": 300,
    "attack": 3,
    "magic": 2,
    "life": 4,
    "lives": 3,
    "keys": 2
  },
  "spells": {
    "shield": true,
    "jump": false,
    "life": false,
    "fairy": false,
    "fire": true,
    "reflect": false,
    "spell": false,
    "thunder": false
  },
  "items": {
    "candle": true,
    "glove": false,
    "raft": false,
    "boots": false,
    "flute": false,
    "cross": false,
    "hammer": true,
    "magic_key": false
  },
  "world": {
    "overworld": 2,
    "world": 4,
    "scene": 36,
    "area": 7,
    "encounter": 2,
    "crystals": 5
  },
  "mode": {
    "mode": 1,
    "state": 1,
    "menu": 0
  },
  "enemies": [
    {
      "slot": 0,
      "id": 10,
      "exists": 1,
      "x": 80,
      "y": 96,
      "page": 0,
      "facing": 1,
      "speed": 3,
      "hp": 7,
      "stun": 0
    },
    {
      "slot": 1,
      "id": 0,
      "exists": 0,
      "x": 0,
      "y": 0,
      "page": 0,
      "facing": 0,
      "speed": 0,
      "hp": 0,
      "stun": 0
    },
    {
      "slot": 2,
      "id": 0,
      "exists": 0,
      "x": 0,
      "y": 0,
      "page": 0,
      "facing": 0,
      "speed": 0,
      "hp": 0,
      "stun": 0
    },
    {
      "slot": 3,
      "id": 0,
      "exists": 0,
      "x": 0,
      "y": 0,
      "page": 0,
      "facing": 0,
      "speed": 0,
      "hp": 0,
      "stun": 0
    },
    {
      "slot": 4,
      "id": 0,
      "exists": 0,
      "x": 0,
      "y": 0,
      "page": 0,
      "facing": 0,
      "speed": 0,
      "hp": 0,
      "stun": 0
    },
    {
      "slot": 5,
      "id": 0,
      "exists": 0,
      "x": 0,
      "y": 0,
      "page": 0,
      "facing": 0,
      "speed": 0,
      "hp": 0,
      "stun": 0
    }
  ],
  "projectiles": [
    {
      "slot": 0,
      "id": 0,
      "x": 0,
      "y": 0,
      "page": 0,
      "facing": 0,
      "speed": 0
    },
    {
      "slot": 1,
      "id": 2,
      "x": 68,
      "y": 112,
      "page": 1,
      "facing": 0,
      "speed": 4
    },
    {
      "slot": 2,
      "id": 0,
      "x": 0,
      "y": 0,
      "page": 0,
      "facing": 0,
      "speed": 0
    },
    {
      "slot": 3,
      "id": 0,
      "x": 0,
      "y": 0,
      "page": 0,
      "facing": 0,
      "speed": 0
    },
    {
      "slot": 4,
      "id": 0,
      "x": 0,
      "y": 0,
      "page": 0,
      "facing": 0,
      "speed": 0
    },
    {
      "slot": 5,
      "id": 0,
      "x": 0,
      "y": 0,
      "page": 0,
      "facing": 0,
      "speed": 0
    }
  ],
  "projectile_flag": 1,
  "timers": {
    "frame": 156,
    "invuln_stun": 0,
    "invuln_blink": 2,
    "kills_easy": 3,
    "kills_hard": 1
  },
  "rng": 165,
  "input": {
    "p1_pressed": 128,
    "p2_pressed": 0,
    "p1_held": 129,
    "p2_held": 0
  }
}"#;

#[test]
fn facts_diff_reports_exactly_changed_leaves() {
    let a = Game::new(synthetic_ram()).facts();
    let same = Game::new(synthetic_ram()).facts();
    assert!(
        facts_diff(&a, &same).is_empty(),
        "identical snapshots must not diff"
    );

    let mut ram = synthetic_ram();
    ram.set_link_hp(0x01); // link.hp: 6 -> 1
    ram.enemy_mut(1).set_hp(0x09); // enemies[1].hp: 0 -> 9
    ram.set_frame_counter(0x9D); // timers.frame: 156 -> 157
    let b = Game::new(ram).facts();
    let diff = facts_diff(&a, &b);
    let rendered: Vec<String> = diff.iter().map(|d| d.to_string()).collect();
    assert_eq!(
        rendered,
        vec![
            "enemies[1].hp: 0 -> 9",
            "link.hp: 6 -> 1",
            "timers.frame: 156 -> 157"
        ],
        "diff must be sorted by path and complete"
    );
}
