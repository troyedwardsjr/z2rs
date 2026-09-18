//! Hitbox overlay data provider.
//!
//! Read-only reuse of `z2_core::player` hitbox builders (`body_box`,
//! `sword_box`, `shield_box`, `boxes_overlap` — bank7 `$E975/$E9A2/$E9D8` /
//! `$E9F9` ports). The provider turns a [`GameFacts`](z2_core::facts::GameFacts)
//! snapshot into screen-space [`OverlayRect`]s; the frontend draws them over
//! its frame (native: egui painter lines; web: same code — no platform
//! calls here).
//!
//! Coordinate note: RAM holds world positions (`link_x` LSB + `link_page`,
//! `link_y`); the overlay maps them to screen pixels with a caller-supplied
//! scroll offset (`screen_x = x.wrapping_sub(scroll_x)`). When the frontend
//! does not know the scroll yet it passes `0` (documented approximation).

use z2_core::facts::GameFacts;
use z2_core::player::{body_box, boxes_overlap, shield_box, sword_box, HitBox};

/// Which box a rect came from (drives outline colour in [`show_layer`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxKind {
    /// Link body (`body_box`).
    Body,
    /// Sword swing (`sword_box`, only while a stab anim is up).
    Sword,
    /// Shield (`shield_box`).
    Shield,
    /// Enemy contact marker (placeholder size — see [`enemy_markers`]).
    Enemy,
}

/// One axis-aligned overlay rect in screen pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlayRect {
    /// Left edge (screen px).
    pub x: f32,
    /// Top edge (screen px).
    pub y: f32,
    /// Width (px).
    pub w: f32,
    /// Height (px).
    pub h: f32,
    /// Source box.
    pub kind: BoxKind,
    /// Short label (e.g. `"body"`, `"e2"`).
    pub label: &'static str,
}

impl OverlayRect {
    /// Outline colour for the canvas.
    #[must_use]
    pub fn color(self) -> egui::Color32 {
        match self.kind {
            BoxKind::Body => egui::Color32::GREEN,
            BoxKind::Sword => egui::Color32::YELLOW,
            BoxKind::Shield => egui::Color32::LIGHT_BLUE,
            BoxKind::Enemy => egui::Color32::RED,
        }
    }

    /// Rect corners for line drawing.
    #[must_use]
    pub fn corners(self) -> [(f32, f32); 4] {
        [
            (self.x, self.y),
            (self.x + self.w, self.y),
            (self.x + self.w, self.y + self.h),
            (self.x, self.y + self.h),
        ]
    }
}

impl From<(HitBox, BoxKind, &'static str)> for OverlayRect {
    fn from((b, kind, label): (HitBox, BoxKind, &'static str)) -> Self {
        Self {
            x: b.x as f32,
            y: b.y as f32,
            w: b.w as f32,
            h: b.h as f32,
            kind,
            label,
        }
    }
}

/// Link `link_facing` values that mean "facing right" on the overlay
/// (ram-map: `1` right, `2` left).
#[must_use]
pub fn facing_is_right(facing: u8) -> bool {
    facing != 2
}

/// `link_sprite` values with an extended sword box (ram-map `$0080` docs:
/// 5 standing stab, 6 crouch stab, 7 crouch stab(?), 8 up stab, 9 down stab;
/// 0-3 walk, 4 wind-up).
#[must_use]
pub fn sword_is_out(sprite: u8) -> bool {
    (5..=9).contains(&sprite)
}

/// Up/down stabs (sprites 8/9) use the second `SWORD_DY`/`SWORD_H` row.
#[must_use]
pub fn is_up_down_stab(sprite: u8) -> bool {
    sprite == 8 || sprite == 9
}

/// Player boxes from a facts snapshot plus Link's sprite byte (`$0080`,
/// via [`link_sprite_of`] on the same snapshot's RAM): body always; sword
/// while a stab anim is up; shield always (caller-supplied `dy`/`h` fold
/// the `LE971`/`LE973` shield-pos tables the engine threads through —
/// defaults `0`/`8` match a standing block; frontends tracking `$17`
/// should pass the real fold).
pub fn player_boxes(
    facts: &GameFacts,
    link_sprite: u8,
    scroll_x: u8,
    shield_dy: u8,
    shield_h: u8,
) -> Vec<OverlayRect> {
    let screen_x = facts.link.x.wrapping_sub(scroll_x);
    let right = facing_is_right(facts.link.facing);
    let mut out = vec![
        OverlayRect::from((
            body_box(screen_x, facts.link.y, right, 0),
            BoxKind::Body,
            "body",
        )),
        OverlayRect::from((
            shield_box(screen_x, facts.link.y, shield_dy, shield_h),
            BoxKind::Shield,
            "shield",
        )),
    ];
    // `sword_box` takes the sword-tip anchor (`$47E/$480`); the snapshot
    // only carries Link's position, so anchor on Link with the engine's own
    // `SWORD_DX` fold (documented approximation — exact anchor bytes are
    // `interp`-only today).
    if sword_is_out(link_sprite) {
        out.push(OverlayRect::from((
            sword_box(screen_x, facts.link.y, right, is_up_down_stab(link_sprite)),
            BoxKind::Sword,
            "sword",
        )));
    }
    out
}

/// Enemy contact markers, one per live slot (`exists == 1|2`, nonzero id).
/// Size is a documented placeholder: per-type contact sizes are ROM-gated
/// (per-type bytes pending), so every marker is 16x16 anchored at
/// the slot `(x, y)` — the NES sprite cell the slot's actor occupies.
/// Returns `(slot, rect)` pairs; `scroll_x` applies as in [`player_boxes`].
pub fn enemy_markers(facts: &GameFacts, scroll_x: u8) -> Vec<(u8, OverlayRect)> {
    const LABELS: [&str; 6] = ["e0", "e1", "e2", "e3", "e4", "e5"];
    let mut out = Vec::new();
    for e in &facts.enemies {
        if (e.exists == 1 || e.exists == 2) && e.id != 0 {
            let b = HitBox {
                x: e.x.wrapping_sub(scroll_x),
                y: e.y,
                w: 16,
                h: 16,
            };
            out.push((
                e.slot,
                OverlayRect::from((b, BoxKind::Enemy, LABELS[e.slot as usize % 6])),
            ));
        }
    }
    out
}

/// Sword-vs-enemy overlap flags for the current boxes (reuse of the engine's
/// own `$E9F9` predicate): `(slot, overlaps)` for each live enemy marker.
#[must_use]
pub fn sword_hits(sword: Option<HitBox>, enemies: &[(u8, HitBox)]) -> Vec<(u8, bool)> {
    match sword {
        None => enemies.iter().map(|(s, _)| (*s, false)).collect(),
        Some(sw) => enemies
            .iter()
            .map(|(s, b)| (*s, boxes_overlap(sw, *b)))
            .collect(),
    }
}

/// Hitbox layer state (visibility toggles only — data stays in the snapshot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HitboxLayer {
    /// Draw player boxes.
    pub show_player: bool,
    /// Draw enemy markers.
    pub show_enemies: bool,
}

impl HitboxLayer {
    /// Both layers on (default overlay state).
    #[must_use]
    pub fn new() -> Self {
        Self {
            show_player: true,
            show_enemies: true,
        }
    }

    /// Draw `rects` as outlines in a 256x240 canvas (line segments only —
    /// no textures, so this runs headless and on wasm unchanged).
    pub fn show_layer(ui: &mut egui::Ui, rects: &[OverlayRect]) {
        // Fixed canvas; 1 rect-px == 1 screen-px at 1x zoom keeps the mapping
        // trivially auditable (frontends scale the whole `Ui` for zoom).
        let (resp, painter) =
            ui.allocate_painter(egui::Vec2::new(256.0, 240.0), egui::Sense::hover());
        let origin = resp.rect.min;
        for r in rects {
            let c = r.corners();
            let pts = [
                egui::Pos2::new(origin.x + c[0].0, origin.y + c[0].1),
                egui::Pos2::new(origin.x + c[1].0, origin.y + c[1].1),
                egui::Pos2::new(origin.x + c[2].0, origin.y + c[2].1),
                egui::Pos2::new(origin.x + c[3].0, origin.y + c[3].1),
            ];
            let stroke = egui::Stroke::new(1.0, r.color());
            for i in 0..4 {
                painter.line_segment([pts[i], pts[(i + 1) % 4]], stroke);
            }
            painter.text(
                pts[0],
                egui::Align2::LEFT_TOP,
                r.label,
                egui::FontId::monospace(9.0),
                r.color(),
            );
        }
    }
}

// --- small accessor ---------------------------------------------------------
// Link's anim byte lives in RAM (`$0080`), not in [`GameFacts`]; callers
// pass it from the same snapshot (`ram.link_sprite()`). Kept as a free
// function so call sites read uniformly.
#[must_use]
pub fn link_sprite_of(ram: &z2_core::ram::Ram) -> u8 {
    ram.link_sprite()
}

#[cfg(test)]
mod tests {
    use super::*;
    use z2_core::facts::Game;
    use z2_core::ram::Ram;

    fn ram_with_link(sprite: u8, facing: u8) -> Ram {
        let mut ram = Ram::new();
        ram.set_link_x(100);
        ram.set_link_y(150);
        ram.set_link_sprite(sprite);
        ram.set_link_facing(facing);
        ram
    }

    #[test]
    fn sword_gate_follows_sprite_byte() {
        assert!(!sword_is_out(0));
        assert!(!sword_is_out(4));
        for s in 5..=9 {
            assert!(sword_is_out(s), "sprite {s}");
        }
        assert!(is_up_down_stab(8));
        assert!(is_up_down_stab(9));
        assert!(!is_up_down_stab(5));
        assert!(facing_is_right(1));
        assert!(!facing_is_right(2));
    }

    #[test]
    fn player_boxes_match_engine_builders() {
        let ram = ram_with_link(5, 1);
        let facts = Game::new(ram.clone()).facts();
        let sprite = link_sprite_of(&ram);
        assert!(sword_is_out(sprite));
        let boxes = player_boxes(&facts, sprite, 0, 0, 8);
        assert_eq!(boxes.len(), 3); // body + shield + sword
        let expected_body = body_box(100, 150, true, 0);
        let body = boxes.iter().find(|b| b.kind == BoxKind::Body).unwrap();
        assert_eq!(
            (body.x as u8, body.y as u8, body.w as u8, body.h as u8),
            (
                expected_body.x,
                expected_body.y,
                expected_body.w,
                expected_body.h
            )
        );
        // No sword when walking.
        let ram = ram_with_link(1, 1);
        let facts = Game::new(ram.clone()).facts();
        assert_eq!(player_boxes(&facts, link_sprite_of(&ram), 0, 0, 8).len(), 2);
    }

    #[test]
    fn scroll_offsets_markers_and_overlap_uses_engine_predicate() {
        let mut ram = ram_with_link(5, 1);
        ram.enemy_mut(0).set_id(0x10);
        ram.enemy_mut(0).set_exists(1);
        ram.enemy_mut(0).set_x(120);
        ram.enemy_mut(0).set_y(150);
        let facts = Game::new(ram.clone()).facts();
        let markers = enemy_markers(&facts, 20);
        assert_eq!(markers.len(), 1);
        assert_eq!((markers[0].1.x as u8, markers[0].1.y as u8), (100, 150));
        // Sword box overlapping the marker must agree with boxes_overlap.
        let sw = sword_box(100, 150, true, false);
        let raw: Vec<(u8, HitBox)> = markers
            .iter()
            .map(|(s, r)| {
                (
                    *s,
                    HitBox {
                        x: r.x as u8,
                        y: r.y as u8,
                        w: r.w as u8,
                        h: r.h as u8,
                    },
                )
            })
            .collect();
        let hits = sword_hits(Some(sw), &raw);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, boxes_overlap(sw, raw[0].1));
    }

    #[test]
    fn headless_canvas_renders_without_panic() {
        let ram = ram_with_link(8, 2);
        let facts = Game::new(ram.clone()).facts();
        let rects = player_boxes(&facts, link_sprite_of(&ram), 0, 0, 8);
        let ctx = egui::Context::default();
        crate::run_headless(&ctx, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                HitboxLayer::show_layer(ui, &rects);
            });
        });
    }
}
