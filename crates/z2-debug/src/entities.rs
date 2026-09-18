//! Enemy / projectile slot tables.
//!
//! Six-slot tables rendered from [`GameFacts`](z2_core::facts::GameFacts)
//! (the `z2-core` facts exporter) — the same structs the headless `--dump` path and the
//! wasm bridge already serialize, so native, web and CI read identical
//! values. Pure reads; no panel here writes state.

use z2_core::facts::{EnemyFacts, GameFacts, ProjectileFacts};

/// One enemy row: the debugging-critical subset
/// (`id`/`exists`/`x`/`y`/`hp`, plus `page`/`facing`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnemyRow {
    /// Slot 0-5.
    pub slot: u8,
    /// Type code (`$00A1 + slot`).
    pub id: u8,
    /// Exists/active flag (`$00B6 + slot`).
    pub exists: u8,
    /// X LSB (`$004E + slot`).
    pub x: u8,
    /// Y (`$002A + slot`).
    pub y: u8,
    /// Map page (`$003C + slot`).
    pub page: u8,
    /// Facing (`$0060 + slot`).
    pub facing: u8,
    /// HP (`$00C2 + slot`).
    pub hp: u8,
}

impl From<&EnemyFacts> for EnemyRow {
    fn from(e: &EnemyFacts) -> Self {
        Self {
            slot: e.slot,
            id: e.id,
            exists: e.exists,
            x: e.x,
            y: e.y,
            page: e.page,
            facing: e.facing,
            hp: e.hp,
        }
    }
}

impl EnemyRow {
    /// Live slot per Data Crystal (`1` = yes; `2` = kill-and-grant-exp).
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.exists == 1 || self.exists == 2
    }
}

/// One projectile row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectileRow {
    /// Slot 0-5.
    pub slot: u8,
    /// Type (`$0087 + slot`).
    pub id: u8,
    /// X (`$0054 + slot`).
    pub x: u8,
    /// Y (`$0030 + slot`).
    pub y: u8,
    /// Map page (`$0042 + slot`).
    pub page: u8,
    /// Facing (`$0066 + slot`).
    pub facing: u8,
}

impl From<&ProjectileFacts> for ProjectileRow {
    fn from(p: &ProjectileFacts) -> Self {
        Self {
            slot: p.slot,
            id: p.id,
            x: p.x,
            y: p.y,
            page: p.page,
            facing: p.facing,
        }
    }
}

/// Enemy rows for all six slots, in slot order.
#[must_use]
pub fn enemy_rows(facts: &GameFacts) -> [EnemyRow; 6] {
    let [a, b, c, d, e, f] = &facts.enemies;
    [a.into(), b.into(), c.into(), d.into(), e.into(), f.into()]
}

/// Projectile rows for all six slots, in slot order.
#[must_use]
pub fn projectile_rows(facts: &GameFacts) -> [ProjectileRow; 6] {
    let [a, b, c, d, e, f] = &facts.projectiles;
    [a.into(), b.into(), c.into(), d.into(), e.into(), f.into()]
}

/// Panel options (display-only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EntitiesPanel {
    /// Hide slots that are not live (`exists == 0` / zero id).
    pub hide_empty: bool,
}

impl EntitiesPanel {
    /// Default panel.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Render both 6-slot tables for `facts`.
    pub fn show(&mut self, ui: &mut egui::Ui, facts: &GameFacts) {
        ui.checkbox(&mut self.hide_empty, "hide empty slots");
        ui.heading("enemies");
        egui::Grid::new("z2_enemy_grid")
            .striped(true)
            .show(ui, |ui| {
                for h in ["slot", "id", "exists", "x", "y", "page", "face", "hp"] {
                    ui.label(h);
                }
                ui.end_row();
                for r in enemy_rows(facts) {
                    if self.hide_empty && !r.is_live() && r.id == 0 {
                        continue;
                    }
                    ui.monospace(r.slot.to_string());
                    ui.monospace(format!("${:02X}", r.id));
                    ui.monospace(format!("${:02X}", r.exists));
                    ui.monospace(format!("${:02X}", r.x));
                    ui.monospace(format!("${:02X}", r.y));
                    ui.monospace(format!("${:02X}", r.page));
                    ui.monospace(format!("${:02X}", r.facing));
                    ui.monospace(format!("${:02X}", r.hp));
                    ui.end_row();
                }
            });
        ui.heading("projectiles");
        ui.monospace(format!("flag=${:02X}", facts.projectile_flag));
        egui::Grid::new("z2_proj_grid")
            .striped(true)
            .show(ui, |ui| {
                for h in ["slot", "id", "x", "y", "page", "face"] {
                    ui.label(h);
                }
                ui.end_row();
                for r in projectile_rows(facts) {
                    if self.hide_empty && r.id == 0 {
                        continue;
                    }
                    ui.monospace(r.slot.to_string());
                    ui.monospace(format!("${:02X}", r.id));
                    ui.monospace(format!("${:02X}", r.x));
                    ui.monospace(format!("${:02X}", r.y));
                    ui.monospace(format!("${:02X}", r.page));
                    ui.monospace(format!("${:02X}", r.facing));
                    ui.end_row();
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use z2_core::facts::Game;
    use z2_core::ram::Ram;

    fn facts_with_enemy() -> GameFacts {
        let mut ram = Ram::new();
        ram.enemy_mut(2).set_id(0x1B);
        ram.enemy_mut(2).set_exists(1);
        ram.enemy_mut(2).set_x(0x80);
        ram.enemy_mut(2).set_y(0xA0);
        ram.enemy_mut(2).set_hp(0x05);
        ram.projectile_mut(0).set_id(0x03);
        Game::new(ram).facts()
    }

    #[test]
    fn rows_carry_slot_order_and_values() {
        let facts = facts_with_enemy();
        let rows = enemy_rows(&facts);
        assert_eq!(rows.len(), 6);
        for (i, r) in rows.iter().enumerate() {
            assert_eq!(r.slot, i as u8);
        }
        assert_eq!(
            rows[2],
            EnemyRow {
                slot: 2,
                id: 0x1B,
                exists: 1,
                x: 0x80,
                y: 0xA0,
                page: 0,
                facing: 0,
                hp: 0x05,
            }
        );
        assert!(rows[2].is_live());
        assert!(!rows[0].is_live());
        let prows = projectile_rows(&facts);
        assert_eq!(prows[0].id, 0x03);
    }

    #[test]
    fn headless_tables_render_without_panic() {
        let facts = facts_with_enemy();
        let mut panel = EntitiesPanel::new();
        panel.hide_empty = true;
        let ctx = egui::Context::default();
        crate::run_headless(&ctx, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                panel.show(ui, &facts);
            });
        });
    }
}
