//! pack.json schema validation, error messages and lookup precedence.

mod common;

use common::{files, page_sheet_png, solid_png};
use z2_render::{HdPack, PackError};

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const CLEAR: [u8; 4] = [0, 0, 0, 0];

/// Scale-1 page sheet with only `tile` painted `px`.
fn one_tile_sheet(tile: u8, px: [u8; 4]) -> Vec<u8> {
    page_sheet_png(1, move |t, _, _| if t == tile { px } else { CLEAR })
}

fn load(json: &str, extra: Vec<(&str, Vec<u8>)>) -> Result<HdPack, PackError> {
    let mut list = vec![("pack.json", json.as_bytes().to_vec())];
    list.extend(extra);
    HdPack::from_files(&files(list))
}

fn err(json: &str, extra: Vec<(&str, Vec<u8>)>) -> PackError {
    load(json, extra).expect_err("pack should be rejected")
}

const MINIMAL: &str = r#"{"format":"z2rs-hdpack","version":1,"name":"t","scale":1,
    "sheets":[{"file":"sheets/p5.png","page":5}]}"#;

#[test]
fn minimal_page_sheet_registers_only_painted_cells() {
    let p = load(MINIMAL, vec![("sheets/p5.png", one_tile_sheet(0x42, RED))]).unwrap();
    assert_eq!((p.name(), p.scale(), p.cell_size()), ("t", 1, 8));
    assert_eq!((p.tile_count(), p.variant_count()), (1, 0));
    let cell = p.lookup(5, 0x42, [1, 2, 3]).expect("tile 0x42 registered");
    assert!(cell.solid && !cell.blank);
    assert_eq!((cell.x0, cell.y0), (2 * 8, 4 * 8));
    let px = p.cell_pixels(cell);
    assert_eq!(px.size(), 8);
    assert_eq!(px.row(3), RED.repeat(8).as_slice());
    assert_eq!(px.to_vec(), RED.repeat(64));
    assert_eq!(p.lookup(5, 0x43, [1, 2, 3]), None);
    assert_eq!(p.lookup(6, 0x42, [1, 2, 3]), None);
    assert_eq!(p.lookup(200, 0, [0; 3]), None);
    assert_eq!(p.page_counts(5), (1, 0));
}

#[test]
fn header_fields_are_validated() {
    let sheet = || vec![("sheets/p5.png", one_tile_sheet(0, RED))];
    assert_eq!(
        err(&MINIMAL.replace("\"version\":1", "\"version\":2"), sheet()),
        PackError::Version { got: 2 }
    );
    for bad in [0, 9] {
        assert_eq!(
            err(
                &MINIMAL.replace("\"scale\":1", &format!("\"scale\":{bad}")),
                sheet()
            ),
            PackError::Scale { got: bad }
        );
    }
    assert!(matches!(
        err(&MINIMAL.replace("z2rs-hdpack", "other"), sheet()),
        PackError::Format { .. }
    ));
    let e = err(&MINIMAL.replace("\"name\":\"t\",", ""), sheet());
    assert!(matches!(e, PackError::Json { .. }));
    assert!(e.to_string().contains("name"), "{e}");
    assert!(matches!(
        err(
            r#"{"version":1,"name":"t","scale":1,"alpha_threshold":300}"#,
            vec![]
        ),
        PackError::AlphaThreshold { got: 300 }
    ));
}

#[test]
fn format_key_is_optional_and_unknown_keys_are_ignored() {
    let json = r#"{"version":1,"name":"t","scale":1,"future_key":{"x":1},
        "sheets":[{"file":"sheets/p5.png","page":5,"artist_note":"hi"}],
        "groups":{"link":{"tiles":[[5,66]]}}}"#;
    let p = load(json, vec![("sheets/p5.png", one_tile_sheet(0x42, RED))]).unwrap();
    assert_eq!(p.tile_count(), 1);
    assert!(p.groups().is_some());
}

#[test]
fn sheet_problems_name_the_file() {
    let e = err(MINIMAL, vec![("sheets/p5.png", solid_png(64, 64, RED))]);
    assert_eq!(
        e,
        PackError::SheetDims {
            file: "sheets/p5.png".into(),
            want: (128, 128),
            got: (64, 64),
            scale: 1
        }
    );
    assert!(e.to_string().contains("sheets/p5.png"));

    let e = err(MINIMAL, vec![]);
    assert!(matches!(&e, PackError::MissingFile { file, .. } if file == "sheets/p5.png"));

    let e = err(MINIMAL, vec![("sheets/p5.png", b"garbage".to_vec())]);
    assert!(matches!(&e, PackError::Png { file, .. } if file == "sheets/p5.png"));
    assert!(e.to_string().starts_with("sheets/p5.png: "), "{e}");

    let json = MINIMAL.replace("\"page\":5", "\"page\":5,\"scale\":2");
    assert!(matches!(
        err(&json, vec![("sheets/p5.png", one_tile_sheet(0, RED))]),
        PackError::ScaleMismatch {
            pack: 1,
            sheet: 2,
            ..
        }
    ));
}

#[test]
fn ranges_and_paths_are_validated() {
    let sheet = || vec![("sheets/p5.png", one_tile_sheet(0, RED))];
    assert!(matches!(
        err(&MINIMAL.replace("\"page\":5", "\"page\":32"), sheet()),
        PackError::Page { got: 32, .. }
    ));
    let e = err(
        &MINIMAL.replace("\"page\":5", "\"page\":5,\"colors\":[1,2,64]"),
        sheet(),
    );
    assert!(matches!(e, PackError::Color { .. }));
    assert!(e.to_string().contains("sheets[0]"), "{e}");
    assert!(matches!(
        err(
            &MINIMAL.replace("\"page\":5", "\"page\":5,\"colors\":[1,2]"),
            sheet()
        ),
        PackError::Color { .. }
    ));
    for bad in ["../p5.png", "/abs/p5.png", "C:\\p5.png"] {
        let json = MINIMAL.replace("sheets/p5.png", &bad.replace('\\', "\\\\"));
        assert!(
            matches!(err(&json, sheet()), PackError::Path { .. }),
            "{bad}"
        );
    }
    let tile_json = |tile: u64| {
        format!(
            r#"{{"version":1,"name":"t","scale":1,"sheets":[{{"file":"free.png"}}],
            "tiles":[{{"page":1,"tile":{tile},"sheet":0,"x":0,"y":0}}]}}"#
        )
    };
    assert!(matches!(
        err(&tile_json(256), vec![("free.png", solid_png(8, 8, RED))]),
        PackError::Tile { got: 256, .. }
    ));
}

#[test]
fn layout_rules() {
    let sheet = || vec![("s.png", one_tile_sheet(0, RED))];
    let base = r#"{"version":1,"name":"t","scale":1,"sheets":[SHEET]}"#;
    for (sheet_json, want) in [
        (r#"{"file":"s.png","layout":"page"}"#, "needs a \"page\""),
        (
            r#"{"file":"s.png","layout":"free","page":1}"#,
            "must not set",
        ),
        (r#"{"file":"s.png","layout":"grid"}"#, "unknown layout"),
    ] {
        let e = err(&base.replace("SHEET", sheet_json), sheet());
        assert!(
            matches!(e, PackError::Layout { .. }) && e.to_string().contains(want),
            "{e}"
        );
    }
}

#[test]
fn duplicates_report_both_sources() {
    let json = r#"{"version":1,"name":"t","scale":1,"sheets":[
        {"file":"a.png","page":5},{"file":"b.png","page":5}]}"#;
    let e = err(
        json,
        vec![
            ("a.png", one_tile_sheet(0x42, RED)),
            ("b.png", one_tile_sheet(0x42, GREEN)),
        ],
    );
    assert!(matches!(
        e,
        PackError::DuplicateTile {
            page: 5,
            tile: 0x42,
            colors: None,
            ..
        }
    ));
    let msg = e.to_string();
    assert!(
        msg.contains("sheets[0]") && msg.contains("sheets[1]"),
        "{msg}"
    );

    let json = r#"{"version":1,"name":"t","scale":1,
        "sheets":[{"file":"a.png","page":5},{"file":"free.png"}],
        "tiles":[{"page":5,"tile":66,"sheet":"free.png","x":0,"y":0}]}"#;
    let e = err(
        json,
        vec![
            ("a.png", one_tile_sheet(0x42, RED)),
            ("free.png", solid_png(8, 8, BLUE)),
        ],
    );
    assert!(e.to_string().contains("tiles[0]"), "{e}");
}

#[test]
fn free_placement_bounds_and_sheet_refs() {
    let extra = || vec![("free.png", solid_png(16, 16, BLUE))];
    let json = |sheet: &str, x: u64| {
        format!(
            r#"{{"version":1,"name":"t","scale":1,"sheets":[{{"file":"free.png"}}],
            "tiles":[{{"page":2,"tile":7,"sheet":{sheet},"x":{x},"y":8}}]}}"#
        )
    };
    let p = load(&json("0", 8), extra()).unwrap();
    let cell = p.lookup(2, 7, [0; 3]).unwrap();
    assert_eq!((cell.x0, cell.y0), (8, 8));
    assert!(load(&json("\"./free.png\"", 8), extra()).is_ok());

    let e = err(&json("0", 12), extra());
    assert!(matches!(
        &e,
        PackError::CellOutOfRange {
            x: 12,
            y: 8,
            cell: 8,
            ..
        }
    ));
    assert!(e.to_string().contains("free.png"), "{e}");
    assert!(matches!(
        err(&json("3", 0), extra()),
        PackError::SheetRef { .. }
    ));
    assert!(matches!(
        err(&json("\"other.png\"", 0), extra()),
        PackError::SheetRef { .. }
    ));
}

#[test]
fn blank_cells_page_vs_explicit() {
    let json = r#"{"version":1,"name":"t","scale":1,
        "sheets":[{"file":"a.png","page":5},{"file":"clear.png"}],
        "tiles":[{"page":6,"tile":1,"sheet":1,"x":0,"y":0}]}"#;
    let p = load(
        json,
        vec![
            ("a.png", page_sheet_png(1, |_, _, _| CLEAR)),
            ("clear.png", solid_png(8, 8, CLEAR)),
        ],
    )
    .unwrap();
    assert_eq!(
        p.lookup_default(5, 0),
        None,
        "empty page cell is not registered"
    );
    let hidden = p
        .lookup(6, 1, [0; 3])
        .expect("explicit entry always registers");
    assert!(hidden.blank && !hidden.solid);
}

#[test]
fn alpha_threshold_binarises_alpha() {
    let half = [10, 20, 30, 127];
    let sheet = || vec![("sheets/p5.png", one_tile_sheet(0, half))];
    let p = load(MINIMAL, sheet()).unwrap();
    assert_eq!(p.tile_count(), 0, "alpha 127 < default threshold 128");
    let json = MINIMAL.replace("\"scale\":1", "\"scale\":1,\"alpha_threshold\":100");
    let p = load(&json, sheet()).unwrap();
    assert_eq!(p.alpha_threshold(), 100);
    let cell = p.lookup(5, 0, [0; 3]).unwrap();
    assert_eq!(p.cell_pixels(cell).pixel(0, 0), [10, 20, 30, 255]);
}

#[test]
fn variant_precedence() {
    let json = r#"{"version":1,"name":"t","scale":1,"sheets":[
        {"file":"default.png","page":5},
        {"file":"variant.png","page":5,"colors":[22,39,48]},
        {"file":"free.png","colors":[7,8,9]}],
      "tiles":[
        {"page":5,"tile":16,"sheet":2,"x":0,"y":0,"colors":[1,2,3],"brightness":1},
        {"page":5,"tile":17,"sheet":2,"x":0,"y":0}]}"#;
    let p = load(
        json,
        vec![
            ("default.png", one_tile_sheet(0x42, RED)),
            ("variant.png", one_tile_sheet(0x42, GREEN)),
            ("free.png", solid_png(8, 8, BLUE)),
        ],
    )
    .unwrap();
    assert_eq!((p.tile_count(), p.variant_count()), (1, 3));
    let px = |c| p.cell_pixels(c).pixel(0, 0);
    assert_eq!(px(p.lookup(5, 0x42, [22, 39, 48]).unwrap()), GREEN);
    // Colours are masked to 6 bits like palette RAM.
    assert_eq!(px(p.lookup(5, 0x42, [22 | 0x40, 39, 48]).unwrap()), GREEN);
    assert_eq!(px(p.lookup(5, 0x42, [22, 39, 49]).unwrap()), RED);
    assert_eq!(px(p.lookup(5, 16, [1, 2, 3]).unwrap()), BLUE);
    assert_eq!(p.lookup(5, 16, [1, 2, 4]), None, "no default: NES fallback");
    // Tile entry without colours inherits the sheet's variant key.
    assert!(p.lookup(5, 17, [7, 8, 9]).is_some());
    assert_eq!(p.lookup(5, 17, [0, 0, 0]), None);
    assert_eq!(p.page_counts(5), (1, 3));
}

#[test]
fn palette_forms() {
    let hex: Vec<String> = (0..64).map(|i| format!("\"#{:02X}0000\"", i)).collect();
    let json = format!(
        r#"{{"version":1,"name":"t","scale":1,"palette":[{}]}}"#,
        hex.join(",")
    );
    let p = load(&json, vec![]).unwrap();
    assert_eq!(p.palette().unwrap().rgb(5), [5, 0, 0]);

    let json = format!(
        r#"{{"version":1,"name":"t","scale":1,"palette":[{}]}}"#,
        hex[..63].join(",")
    );
    assert!(matches!(err(&json, vec![]), PackError::Palette { .. }));

    let triples: Vec<String> = (0..64).map(|i| format!("[0,{i},0]")).collect();
    let json = format!(
        r#"{{"version":1,"name":"t","scale":1,"palette":[{}]}}"#,
        triples.join(",")
    );
    assert_eq!(
        load(&json, vec![]).unwrap().palette().unwrap().rgb(9),
        [0, 9, 0]
    );

    let json = r#"{"version":1,"name":"t","scale":1,"palette":"my.pal"}"#;
    let pal: Vec<u8> = (0..192).map(|i| i as u8).collect();
    let p = load(json, vec![("my.pal", pal)]).unwrap();
    assert_eq!(p.palette().unwrap().rgb(1), [3, 4, 5]);
    let e = err(json, vec![("my.pal", vec![0; 100])]);
    assert!(matches!(e, PackError::Palette { .. }) && e.to_string().contains("my.pal"));
    assert!(load(r#"{"version":1,"name":"t","scale":1}"#, vec![])
        .unwrap()
        .palette()
        .is_none());
}

#[test]
fn directory_prefixes_are_stripped() {
    let json = MINIMAL.as_bytes().to_vec();
    for sep in ["/", "\\"] {
        let list = vec![
            (format!("myfolder{sep}pack.json"), json.clone()),
            (
                format!("myfolder{sep}sheets{sep}p5.png"),
                one_tile_sheet(0x42, RED),
            ),
            // A nested scratch pack.json must not win over the root one.
            (format!("myfolder{sep}old{sep}pack.json"), b"{}".to_vec()),
        ];
        let p = HdPack::from_files(&list).unwrap();
        assert_eq!(p.tile_count(), 1);
    }
    assert!(matches!(
        HdPack::from_files(&[]),
        Err(PackError::MissingFile { .. })
    ));
}

#[test]
fn scale_two_cells() {
    let json = MINIMAL.replace("\"scale\":1", "\"scale\":2");
    let sheet = page_sheet_png(2, |t, x, _| {
        if t == 3 && x < 8 {
            RED
        } else if t == 3 {
            CLEAR
        } else {
            [0; 4]
        }
    });
    let p = load(&json, vec![("sheets/p5.png", sheet)]).unwrap();
    let cell = p.lookup(5, 3, [0; 3]).unwrap();
    assert!(!cell.solid);
    let px = p.cell_pixels(cell);
    assert_eq!(px.size(), 16);
    assert_eq!(px.row(15).len(), 64);
    assert_eq!(px.pixel(7, 15), RED);
    assert_eq!(px.pixel(8, 15)[3], 0);
}

#[test]
fn sprite_alpha_and_bleed_are_validated() {
    let free = |tiles: &str, alpha: &str| {
        format!(
            r#"{{"version":1,"name":"t","scale":2,{alpha}"sheets":[{{"file":"s.png"}}],"tiles":[{tiles}]}}"#
        )
    };
    // A 40x24 sheet at scale 2: one 16 px core with up to 12 px around it.
    let sheet = || vec![("s.png", solid_png(40, 24, GREEN))];
    let tile = r#"{"page":3,"tile":7,"sheet":0,"x":8,"y":4,"bleed":[4,2,8,2]}"#;

    let p = load(&free(tile, r#""sprite_alpha":"art","#), sheet()).unwrap();
    assert!(p.sprite_alpha_art());
    assert_eq!(p.max_bleed_v(), 2);
    let cell = p.lookup(3, 7, [0; 3]).unwrap();
    assert_eq!(cell.bleed, [4, 2, 8, 2]);
    let px = p.cell_pixels(cell);
    assert_eq!(
        px.pixel_rel(-8, -4),
        Some(GREEN),
        "bleed is in sheet pixels around the core"
    );
    assert_eq!(px.pixel_rel(31, 19), Some(GREEN));
    assert_eq!(
        (
            px.pixel_rel(-9, 0),
            px.pixel_rel(32, 0),
            px.pixel_rel(0, 20)
        ),
        (None, None, None)
    );

    let p = load(&free(tile, r#""sprite_alpha":"nes","#), sheet()).unwrap();
    assert!(!p.sprite_alpha_art());
    assert_eq!(
        err(&free(tile, r#""sprite_alpha":"hd","#), sheet()),
        PackError::SpriteAlpha { got: "hd".into() }
    );

    for (bad, want) in [
        (r#""bleed":[1,2,3]"#, "has 3 values"),
        (r#""bleed":[0,0,9,0]"#, "value 9 out of range (0..=8)"),
        (
            r#""bleed":[5,0,0,0]"#,
            "does not fit inside \"s.png\" (40x24 px)",
        ),
        (r#""bleed":[0,0,0,3]"#, "does not fit inside"),
    ] {
        let json = free(&tile.replace(r#""bleed":[4,2,8,2]"#, bad), "");
        let e = err(&json, sheet());
        assert!(matches!(e, PackError::Bleed { .. }), "{bad}: {e:?}");
        assert!(e.to_string().contains(want), "{bad}: {e}");
        assert!(
            e.to_string().starts_with("pack.json tiles[0]: bleed "),
            "{e}"
        );
    }
}

#[test]
fn layers_are_validated_and_resolved() {
    use z2_render::{LayerDepth, SceneView};
    let pack =
        |layers: &str| format!(r#"{{"version":1,"name":"t","scale":2,"layers":[{layers}]}}"#);
    let art = || {
        vec![
            ("l/far.png", solid_png(64, 32, BLUE)),
            ("l/near.png", solid_png(8, 8, RED)),
        ]
    };

    let p = load(
        &pack(
            r#"{"file":"l/far.png","x":-66,"scroll":20,"repeat_x":true,"when":{"world":1,"scene":7}},
               {"file":"l/near.png","depth":"front","x":22,"y":208,
                "over_tiles":[{"page":7,"tile":75},{"page":7,"tile":3},{"page":7,"tile":75}]}"#,
        ),
        art(),
    )
    .unwrap();
    let [far, near] = p.layers() else {
        panic!("two layers")
    };
    assert_eq!(
        (far.depth, far.x, far.y, far.scroll, far.repeat_x),
        (LayerDepth::Back, -66, 0, 20, true)
    );
    assert_eq!((far.world, far.region, far.scene), (Some(1), None, Some(7)));
    assert_eq!(
        (near.depth, near.scroll, near.repeat_x),
        (LayerDepth::Front, 100, false)
    );
    assert_eq!(
        near.over_tiles,
        vec![(7, 3), (7, 75)],
        "sorted, deduplicated"
    );
    assert!(near.covers_tile(7, 75) && !near.covers_tile(7, 76));
    assert!(far.covers_tile(0, 0), "no list covers every tile");
    assert_eq!(p.sheet_file(usize::from(far.sheet)), Some("l/far.png"));

    let scene = |world, scene| SceneView {
        world,
        region: 0,
        scene,
        camera_x: 0,
    };
    assert!(far.matches(Some(&scene(1, 7))));
    assert!(!far.matches(Some(&scene(1, 8))) && !far.matches(Some(&scene(2, 7))));
    assert!(!far.matches(None), "a layer that names a scene needs one");
    assert!(near.matches(None) && near.matches(Some(&scene(5, 5))));

    for (bad, want) in [
        (
            r#"{"file":"l/far.png","depth":"middle"}"#,
            "unknown depth \"middle\"",
        ),
        (
            r#"{"file":"l/far.png","scroll":101}"#,
            "scroll 101 out of range (0..=100)",
        ),
        (r#"{"file":"l/far.png","x":70000}"#, "x 70000 out of range"),
        (
            r#"{"file":"l/far.png","when":{"scene":256}}"#,
            "when.scene 256 out of range",
        ),
        (
            r#"{"file":"l/far.png","over_tiles":[{"page":7,"tile":75}]}"#,
            "\"over_tiles\" needs depth \"front\"",
        ),
    ] {
        let e = err(&pack(bad), art());
        assert!(matches!(e, PackError::Layer { .. }), "{bad}: {e:?}");
        assert!(e.to_string().contains(want), "{bad}: {e}");
        assert!(
            e.to_string()
                .starts_with("pack.json layers[0] (\"l/far.png\"): "),
            "{e}"
        );
    }
    assert!(matches!(
        err(&pack(r#"{"file":"l/missing.png"}"#), art()),
        PackError::MissingFile { .. }
    ));
    assert!(matches!(
        err(&pack(r#"{"file":"../far.png"}"#), art()),
        PackError::Path { .. }
    ));
    assert!(matches!(
        err(
            &pack(r#"{"file":"l/near.png","depth":"front","over_tiles":[{"page":32,"tile":0}]}"#),
            art()
        ),
        PackError::Page { got: 32, .. }
    ));
}
