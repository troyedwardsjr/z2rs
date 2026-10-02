//! The "Enhancements (ZALiA-inspired)" option widgets, shared by the
//! launcher (`z2-launcher`) and the game's in-game options overlay
//! (`z2-native`, the `O` key).
//!
//! One collapsing header per option group; every option has a tooltip saying
//! what it does and where it comes from in ZALiA (HoverBat's Zelda II remake,
//! used only as a feature reference). Everything is off by default, which is
//! the original game; see README.md for which options are
//! implemented yet.
//!
//! Every function draws into a plain `&mut egui::Ui` and edits the
//! `z2_core::enh` option types in place, so the two frontends share one
//! version of the widgets and one JSON shape. The caller decides when an edit
//! takes effect (the launcher saves it, the overlay applies it to the running
//! game). [`Opts::gameplay_editable`] greys out every gameplay option while
//! leaving the groups expandable, which is how the overlay shows the
//! session's options read-only during netplay or movie playback.

use egui::RichText;
use z2_core::enh::{
    abilities::MAX_SWORD_REACH_PX,
    display::MAX_VOLUME,
    rando::{MAX_PALETTE_RANDO, MAX_SCALE_PCT, MAX_START_LEVEL},
    text::MAX_DIALOGUE_SPEED,
    ContinueFrom, DisplayEnh, Enhancements, FlashColor,
};

/// Spell bit names for [`z2_core::enh::RandoOpts::start_spells`].
const SPELLS: [&str; 8] = [
    "Shield", "Jump", "Life", "Fairy", "Fire", "Reflect", "Spell", "Thunder",
];
/// Item / skill bit names for [`z2_core::enh::RandoOpts::start_items`].
const ITEMS: [&str; 10] = [
    "Candle",
    "Glove",
    "Raft",
    "Boots",
    "Flute",
    "Cross",
    "Hammer",
    "Magic key",
    "Downward thrust",
    "Upward thrust",
];

/// How the groups draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opts {
    /// Open every group (tests use it so every control renders).
    pub force_open: bool,
    /// Whether gameplay options may be edited. Display options always may.
    pub gameplay_editable: bool,
    /// The ROM randomizer (`z2-rando`, the launcher's Randomizer tab) is on:
    /// options that assume the original world (the item shuffle) are greyed
    /// out with a note, because the game turns them off
    /// ([`Enhancements::for_rom`]).
    pub rom_randomized: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            force_open: false,
            gameplay_editable: true,
            rom_randomized: false,
        }
    }
}

/// One option group, in launcher order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// Text & HUD (gameplay, plus the display-only quest timer).
    Text,
    /// Quality of life.
    Qol,
    /// Engine fixes.
    Fixes,
    /// Enemies & bosses.
    Enemies,
    /// Abilities.
    Abilities,
    /// Randomizer & start.
    Rando,
    /// Cheats.
    Cheats,
    /// Graphics effects (display only).
    Graphics,
    /// Audio (display only).
    Audio,
    /// Dev tools (display only).
    Dev,
}

impl Group {
    /// Every group, in launcher order.
    pub const ALL: [Group; 10] = [
        Group::Text,
        Group::Qol,
        Group::Fixes,
        Group::Enemies,
        Group::Abilities,
        Group::Rando,
        Group::Cheats,
        Group::Graphics,
        Group::Audio,
        Group::Dev,
    ];

    /// Whether the group holds gameplay options (the text group holds both).
    #[must_use]
    pub fn is_gameplay(self) -> bool {
        !matches!(self, Group::Graphics | Group::Audio | Group::Dev)
    }
}

/// Draw one group.
pub fn group_ui(ui: &mut egui::Ui, g: Group, e: &mut Enhancements, d: &mut DisplayEnh, o: Opts) {
    match g {
        Group::Text => text_group(ui, e, d, o),
        Group::Qol => qol_group(ui, e, o),
        Group::Fixes => fixes_group(ui, e, o),
        Group::Enemies => enemies_group(ui, e, o),
        Group::Abilities => abilities_group(ui, e, o),
        Group::Rando => rando_group(ui, e, o),
        Group::Cheats => cheats_group(ui, e, o),
        Group::Graphics => graphics_group(ui, d, o),
        Group::Audio => audio_group(ui, d, o),
        Group::Dev => dev_group(ui, d, o),
    }
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).weak());
}

fn check(ui: &mut egui::Ui, v: &mut bool, label: &str, tip: &str) {
    ui.checkbox(v, label).on_hover_text(tip);
}

fn slider_u8(
    ui: &mut egui::Ui,
    v: &mut u8,
    range: std::ops::RangeInclusive<u8>,
    label: &str,
    tip: &str,
) {
    ui.horizontal(|ui| {
        ui.label(label).on_hover_text(tip);
        ui.add(egui::Slider::new(v, range)).on_hover_text(tip);
    });
}

fn slider_pct(ui: &mut egui::Ui, v: &mut i8, label: &str, tip: &str) {
    ui.horizontal(|ui| {
        ui.label(label).on_hover_text(tip);
        ui.add(egui::Slider::new(v, -MAX_SCALE_PCT..=MAX_SCALE_PCT).suffix("%"))
            .on_hover_text(tip);
    });
}

fn slider_f32(
    ui: &mut egui::Ui,
    v: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    label: &str,
    tip: &str,
) {
    ui.horizontal(|ui| {
        ui.label(label).on_hover_text(tip);
        ui.add(egui::Slider::new(v, range).fixed_decimals(3))
            .on_hover_text(tip);
    });
}

/// Checkboxes for the low `names.len()` bits of `mask`.
fn bit_boxes(ui: &mut egui::Ui, mask: &mut u16, names: &[&str], tip: &str) {
    ui.horizontal_wrapped(|ui| {
        for (i, name) in names.iter().enumerate() {
            let bit = 1u16 << i;
            let mut on = *mask & bit != 0;
            if ui.checkbox(&mut on, *name).on_hover_text(tip).changed() {
                if on {
                    *mask |= bit;
                } else {
                    *mask &= !bit;
                }
            }
        }
    });
}

/// A collapsing group whose body is greyed out (but still visible) when
/// `editable` is false.
fn group(
    ui: &mut egui::Ui,
    title: &str,
    salt: &str,
    force_open: bool,
    editable: bool,
    body: impl FnOnce(&mut egui::Ui),
) {
    egui::CollapsingHeader::new(title)
        .id_salt(salt)
        .open(force_open.then_some(true))
        .show(ui, |ui| {
            ui.add_enabled_ui(editable, body);
        });
}

/// The intro text and status line of the section.
pub fn intro(ui: &mut egui::Ui, e: &Enhancements) {
    hint(
        ui,
        "Optional changes inspired by ZALiA, HoverBat's Zelda II remake. Everything is off \
         by default, which is the original game. Gameplay options change the game: online \
         partners must pick exactly the same, and movie playback ignores them. Hover an \
         option for details. Options not implemented yet are saved but have no effect.",
    );
    let status = if e.any_gameplay_active() {
        "Gameplay enhancements are on: online partners must use the same ones."
    } else {
        "Gameplay: original game."
    };
    hint(ui, status);
}

/// "Reset to original" and "ZALiA preset". With `gameplay_editable` false
/// they only touch the display options.
pub fn preset_buttons(
    ui: &mut egui::Ui,
    e: &mut Enhancements,
    d: &mut DisplayEnh,
    gameplay_editable: bool,
) {
    ui.horizontal(|ui| {
        if ui
            .button("Reset to original")
            .on_hover_text("Turn every enhancement off (the original NES game).")
            .clicked()
        {
            if gameplay_editable {
                *e = Enhancements::default();
            }
            *d = DisplayEnh::default();
        }
        if ui
            .button("ZALiA preset")
            .on_hover_text(
                "ZALiA's own defaults: dialogue speed 2, continue from the palace entrance \
                 or last town, keep 25% XP, lives from dolls, enter towns from the side, \
                 wise men restore MP, every engine fix and enemy tweak, screen shake and a \
                 red flash.",
            )
            .clicked()
        {
            if gameplay_editable {
                *e = Enhancements::zalia_preset();
            }
            *d = DisplayEnh::zalia_preset();
        }
    });
}

/// Draw the whole section (heading, intro, presets and every group), as the
/// launcher shows it.
pub fn enhancements_section(ui: &mut egui::Ui, e: &mut Enhancements, d: &mut DisplayEnh, o: Opts) {
    ui.add_space(6.0);
    ui.heading("Enhancements (ZALiA-inspired)");
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        intro(ui, e);
        preset_buttons(ui, e, d, o.gameplay_editable);
        for g in Group::ALL {
            group_ui(ui, g, e, d, o);
        }
    });
}

fn text_group(ui: &mut egui::Ui, e: &mut Enhancements, d: &mut DisplayEnh, o: Opts) {
    group(ui, "Text & HUD", "enh-text", o.force_open, true, |ui| {
        ui.add_enabled_ui(o.gameplay_editable, |ui| {
            slider_u8(
                ui,
                &mut e.text.dialogue_speed,
                0..=MAX_DIALOGUE_SPEED,
                "Dialogue speed",
                "0 = original speed, 5 = instant. ZALiA: mod_DLG_SPEED (default 2); the delay \
                 between letters drops from 5 frames to 4, 3, 2, 1, 0.",
            );
            check(
                ui,
                &mut e.text.protect_spell_name,
                "Call the SHIELD spell PROTECT",
                "Renames SHIELD to PROTECT in the spell menu. ZALiA: mod_SpellSHIELD_NAME.",
            );
        });
        check(
            ui,
            &mut d.quest_timer,
            "Show the quest timer",
            "Speedrun-style 00:00:00.00 timer since the quest started (display only). \
             ZALiA: Options > Other > Quest timer.",
        );
    });
}

fn qol_group(ui: &mut egui::Ui, e: &mut Enhancements, o: Opts) {
    group(
        ui,
        "Quality of life",
        "enh-qol",
        o.force_open,
        o.gameplay_editable,
        |ui| {
            let q = &mut e.qol;
            let tip = "Where you continue after a game over. Original: always the North Palace. \
                   ZALiA (mod_ContinueFrom): the palace entrance after dying in a palace, the \
                   last town you saved in after dying elsewhere.";
            ui.horizontal(|ui| {
                ui.label("Continue from").on_hover_text(tip);
                egui::ComboBox::from_id_salt("enh-continue")
                    .selected_text(q.continue_from.label())
                    .show_ui(ui, |ui| {
                        for c in ContinueFrom::ALL {
                            ui.selectable_value(&mut q.continue_from, c, c.label());
                        }
                    })
                    .response
                    .on_hover_text(tip);
            });
            slider_u8(
                ui,
                &mut q.gameover_keep_xp_pct,
                0..=100,
                "XP kept on game over (%)",
                "0 = original (all XP lost). ZALiA keeps 25% (mod_Gameover_XP_PENALTY).",
            );
            check(
                ui,
                &mut q.lives_from_dolls,
                "Lives from Life Dolls",
                "Lives start at 3 plus one per Life Doll found, so dolls count for good. \
             ZALiA: mod_START_RUN_LIVES.",
            );
            check(
                ui,
                &mut q.enter_town_from_side,
                "Enter towns from the side you came from",
                "Start at the left or right end of a town depending on your approach. The \
             original always starts at the right end. ZALiA: mod_EnterTownSideOption.",
            );
            check(
                ui,
                &mut q.wise_men_restore_mp,
                "Wise men restore MP",
                "A wise man refills your magic when he teaches his spell. ZALiA: \
             mod_WISEMEN_RESTORE_MP.",
            );
            check(
                ui,
                &mut q.no_mp_requirement_for_spells,
                "No magic requirement to learn spells",
                "Learn a spell without the minimum number of magic containers. ZALiA: \
             mod_AcquireSpellRequirement.",
            );
            check(
                ui,
                &mut q.overworld_softlock_warp,
                "Overworld softlock warp",
                "Hold a button chord for 3 seconds while paused on the overworld to warp to the \
             continue screen. ZALiA's softlock failsafe.",
            );
        },
    );
}

fn fixes_group(ui: &mut egui::Ui, e: &mut Enhancements, o: Opts) {
    group(
        ui,
        "Engine fixes",
        "enh-fixes",
        o.force_open,
        o.gameplay_editable,
        |ui| {
            let f = &mut e.fixes;
            check(
                ui,
                &mut f.levelup_softlocks,
                "Level-up softlock fixes",
                "The level-up window can no longer open during a room change or a dialogue. \
             ZALiA: mod_FIX_SOFTLOCK_LVLUP_1/2.",
            );
            check(
                ui,
                &mut f.iframe_update_skip,
                "No skipped updates while invincible",
                "The original skips camera and collision updates on alternate frames when the \
             invincibility timer is 1 or 2. ZALiA: mod_PCUpdate1.",
            );
            check(
                ui,
                &mut f.jump_direction_balance,
                "Same jump in both directions",
                "Full jump speed is slightly easier to reach moving right in the original. \
             ZALiA: mod_PCJumpDirBalancing.",
            );
            check(
                ui,
                &mut f.shield_hitbox_symmetry,
                "Same shield hitbox both ways",
                "The right-facing shield hitbox sits 2 px further out in the original. ZALiA: \
             PC_init.",
            );
            check(
                ui,
                &mut f.crumble_both_feet,
                "Crumbling bridges check both feet",
                "Crumble tiles react to either foot, not only the middle point. ZALiA: \
             mod_CRUMBLE_TILES.",
            );
            check(
                ui,
                &mut f.xp_drain_fix,
                "XP drain fix",
                "With 10 or less XP pending, the counter drains one at a time and leaves no \
             leftover. ZALiA: update_xp.",
            );
        },
    );
}

fn enemies_group(ui: &mut egui::Ui, e: &mut Enhancements, o: Opts) {
    group(
        ui,
        "Enemies & bosses",
        "enh-enemies",
        o.force_open,
        o.gameplay_editable,
        |ui| {
            let n = &mut e.enemies;
            let rows: [(&mut bool, &str, &str); 10] = [
                (
                    &mut n.ironknuckle_aggro,
                    "Iron Knuckle aggro rules",
                    "Off-screen Iron Knuckles must face Link to attack and give up once he is \
                 below them. ZALiA: mod_IronKnuckle_AggroAI.",
                ),
                (
                    &mut n.ra_hp_reduced,
                    "Ra with less HP",
                    "The flying Ra heads take fewer hits. ZALiA: mod_Ra_HP.",
                ),
                (
                    &mut n.stalfos_upthrust_fix,
                    "Stalfos upthrust fix",
                    "The Stalfos upthrust immunity no longer swallows your other sword hits. \
                 ZALiA: mod_STALFOS_CONTROL1.",
                ),
                (
                    &mut n.wizard_teleport_wide,
                    "Wizards teleport anywhere",
                    "Wizards teleport across the whole room, spread out, instead of the first \
                 screen only. ZALiA: mod_Wizard_TELEPORT_AREA.",
                ),
                (
                    &mut n.mago_balance,
                    "Mago balance",
                    "Mago shots spawn evenly on both sides and a lone Mago teleports closer. \
                 ZALiA: Mago ADJ1/ADJ3.",
                ),
                (
                    &mut n.boss_first_attack_delay,
                    "Bosses wait before attacking",
                    "Carock, Gooma, Helmethead and Horsehead pause before their first attack. \
                 ZALiA boss changes.",
                ),
                (
                    &mut n.helmethead_fix,
                    "Helmethead head fix",
                    "Helmethead's flying head rises back to the right height. ZALiA: \
                 mod_FenserFix1.",
                ),
                (
                    &mut n.carock_longer_vuln,
                    "Carock vulnerable longer",
                    "A longer window to hit Carock. ZALiA: Carock VulnDur.",
                ),
                (
                    &mut n.barba_aim,
                    "Barba aims",
                    "Barba's fire is aimed at Link. ZALiA: Barba Aim.",
                ),
                (
                    &mut n.p5_horsehead,
                    "Horsehead in Palace 5's false-wall room",
                    "As in the Japanese FDS version (the US game has a blue Iron Knuckle). ZALiA: \
                 mod_P5HorseHead.",
                ),
            ];
            for (v, label, tip) in rows {
                check(ui, v, label, tip);
            }
        },
    );
}

fn abilities_group(ui: &mut egui::Ui, e: &mut Enhancements, o: Opts) {
    group(
        ui,
        "Abilities",
        "enh-abilities",
        o.force_open,
        o.gameplay_editable,
        |ui| {
            let a = &mut e.abilities;
            check(
                ui,
                &mut a.double_jump,
                "Double jump",
                "One extra jump in mid-air. ZALiA: the FEATHER item.",
            );
            check(
                ui,
                &mut a.stab_frenzy,
                "Stab frenzy",
                "Hold B to stab over and over. ZALiA: Options > Other > Stab frenzy.",
            );
            slider_u8(
                ui,
                &mut a.sword_reach_px,
                0..=MAX_SWORD_REACH_PX,
                "Extra sword reach (px)",
                "0 = original. ZALiA's SWORD item adds 3 px (mod_PCSword2).",
            );
            slider_u8(
                ui,
                &mut a.damage_reduction_pct,
                0..=100,
                "Damage reduction (%)",
                "0 = original. ZALiA's RING halves damage; RING with PROTECT takes a third.",
            );
            check(
                ui,
                &mut a.mp_regen,
                "Slow magic regeneration",
                "+2 MP every 128 frames. ZALiA: the PENDANT item.",
            );
            check(
                ui,
                &mut a.reflect_more,
                "Stronger REFLECT",
                "REFLECT bounces more kinds of shots, and bounced shots hurt enemies. ZALiA: \
             mod_REFLECT_more_obj.",
            );
            check(
                ui,
                &mut a.dash_speed,
                "Dash",
                "Run faster on the ground while holding the dash button. ZALiA: Dev tools > \
             Faster movement speed.",
            );
            check(
                ui,
                &mut a.rescue_fairy,
                "Rescue fairy",
                "Falling into a pit or lava puts you back on safe ground instead of costing a \
             life. ZALiA: the RESCUE FAIRY item.",
            );
            check(
                ui,
                &mut a.flute_warp,
                "Flute warp",
                "On the overworld, Select + B cycles through towns you have visited. ZALiA: \
             randomizer flute warping.",
            );
        },
    );
}

fn rando_group(ui: &mut egui::Ui, e: &mut Enhancements, o: Opts) {
    group(
        ui,
        "Randomizer & start",
        "enh-rando",
        o.force_open,
        o.gameplay_editable,
        |ui| {
            let r = &mut e.rando;
            ui.horizontal(|ui| {
                let tip = "Seed for the shuffles. Same seed, same game. ZALiA: File Select > \
                       RANDO > EDIT SEED.";
                ui.label("Seed").on_hover_text(tip);
                ui.add(egui::DragValue::new(&mut r.seed)).on_hover_text(tip);
            });
            let lvl = "0 = original (level 1). ZALiA: RANDO > START ATTACK/MAGIC/LIFE.";
            slider_u8(
                ui,
                &mut r.start_attack,
                0..=MAX_START_LEVEL,
                "Start attack level",
                lvl,
            );
            slider_u8(
                ui,
                &mut r.start_magic,
                0..=MAX_START_LEVEL,
                "Start magic level",
                lvl,
            );
            slider_u8(
                ui,
                &mut r.start_life,
                0..=MAX_START_LEVEL,
                "Start life level",
                lvl,
            );
            let cont = "0 = original (4). ZALiA: RANDO > START ITEMS containers.";
            slider_u8(
                ui,
                &mut r.start_containers_heart,
                0..=MAX_START_LEVEL,
                "Start heart containers",
                cont,
            );
            slider_u8(
                ui,
                &mut r.start_containers_magic,
                0..=MAX_START_LEVEL,
                "Start magic containers",
                cont,
            );
            ui.label("Start spells");
            let mut spells = u16::from(r.start_spells);
            bit_boxes(
                ui,
                &mut spells,
                &SPELLS,
                "Spells known from the start. ZALiA: RANDO > START SPELLS.",
            );
            r.start_spells = spells as u8;
            ui.label("Start items and skills");
            bit_boxes(
                ui,
                &mut r.start_items,
                &ITEMS,
                "Items and sword skills owned from the start. ZALiA: RANDO > START ITEMS / \
             START SKILLS.",
            );
            slider_pct(
                ui,
                &mut r.enemy_hp_pct,
                "Enemy HP",
                "0 = original. ZALiA: RANDO > ENEMY HP +/-25%.",
            );
            slider_pct(
                ui,
                &mut r.enemy_dmg_pct,
                "Enemy damage",
                "0 = original. ZALiA: RANDO > ENEMY DAMAGE +/-25%.",
            );
            slider_pct(
                ui,
                &mut r.xp_pct,
                "XP gained",
                "0 = original. ZALiA: RANDO > XP +/-25%.",
            );
            slider_pct(
                ui,
                &mut r.level_cost_pct,
                "Level-up cost",
                "0 = original. ZALiA: RANDO > LEVEL COSTS +/-25%.",
            );
            slider_pct(
                ui,
                &mut r.spell_cost_pct,
                "Spell cost",
                "0 = original. ZALiA: RANDO > SPELL COSTS.",
            );
            slider_u8(
                ui,
                &mut r.palette_rando,
                0..=MAX_PALETTE_RANDO,
                "Palette randomizer",
                "0 off, 1 Link and palaces, 2 every scene (stable per seed). ZALiA: PALETTE \
             RANDO.",
            );
            ui.add_enabled_ui(!o.rom_randomized, |ui| {
                check(
                    ui,
                    &mut r.item_shuffle,
                    "Shuffle item locations",
                    "Items are placed at random, always beatable. ZALiA: RANDO > ITEMS.",
                );
            });
            if o.rom_randomized {
                hint(
                    ui,
                    "Item shuffle is off while the ROM randomizer is on: that seed already \
                     placed the items, and this shuffle assumes the original locations. \
                     The other options here apply on top of the seed.",
                );
            }
        },
    );
}

fn cheats_group(ui: &mut egui::Ui, e: &mut Enhancements, o: Opts) {
    group(
        ui,
        "Cheats",
        "enh-cheats",
        o.force_open,
        o.gameplay_editable,
        |ui| {
            let c = &mut e.cheats;
            check(
                ui,
                &mut c.invincible,
                "Invincible",
                "Link takes no damage. ZALiA: dev invulnerability (I key).",
            );
            check(
                ui,
                &mut c.infinite_magic,
                "Infinite magic",
                "Magic never runs out. ZALiA: dev stab-to-cheat jars.",
            );
            check(
                ui,
                &mut c.infinite_lives,
                "Infinite lives",
                "The life counter never goes down.",
            );
            check(
                ui,
                &mut c.max_stats,
                "Max stats",
                "Attack, magic and life at level 8 with every container. ZALiA: dev \
             stab-to-cheat levels and containers.",
            );
        },
    );
}

fn graphics_group(ui: &mut egui::Ui, d: &mut DisplayEnh, o: Opts) {
    group(
        ui,
        "Graphics effects",
        "enh-graphics",
        o.force_open,
        true,
        |ui| {
            hint(ui, "Display only: these never change the game.");
            check(
                ui,
                &mut d.screen_shake,
                "Screen shake",
                "Shake the picture on boss explosions and big hits. ZALiA: Options > Other > \
             Screen shake (on there).",
            );
            let tip = "Colour the background flashes on spell casts and explosions. None turns \
                   the flashing off (easier on the eyes). ZALiA: Background flashing color \
                   (red there).";
            ui.horizontal(|ui| {
                ui.label("Flash colour").on_hover_text(tip);
                egui::ComboBox::from_id_salt("enh-flash")
                    .selected_text(d.flash_color.label())
                    .show_ui(ui, |ui| {
                        for c in FlashColor::ALL {
                            ui.selectable_value(&mut d.flash_color, c, c.label());
                        }
                    })
                    .response
                    .on_hover_text(tip);
            });
            check(
                ui,
                &mut d.effects_enabled,
                "Post effects",
                "Turn on the five effects below. ZALiA: Graphics Effects Editor (off there by \
             default).",
            );
            ui.add_enabled_ui(d.effects_enabled, |ui| {
                slider_f32(
                    ui,
                    &mut d.brightness,
                    -1.0..=1.0,
                    "Brightness",
                    "ZALiA GEE default 0.05.",
                );
                slider_f32(
                    ui,
                    &mut d.saturation,
                    -1.0..=1.0,
                    "Saturation",
                    "ZALiA GEE default -0.075.",
                );
                slider_f32(
                    ui,
                    &mut d.scanlines,
                    0.0..=1.0,
                    "Scanlines",
                    "ZALiA GEE default 0.5.",
                );
                slider_f32(ui, &mut d.bloom, 0.0..=1.0, "Bloom", "ZALiA GEE bloom.");
                slider_f32(
                    ui,
                    &mut d.blur,
                    0.0..=1.0,
                    "Blur",
                    "ZALiA GEE default 0.85.",
                );
            });
        },
    );
}

fn audio_group(ui: &mut egui::Ui, d: &mut DisplayEnh, o: Opts) {
    group(ui, "Audio", "enh-audio", o.force_open, true, |ui| {
        slider_u8(
            ui,
            &mut d.music_volume,
            0..=MAX_VOLUME,
            "Music volume",
            "10 = unchanged. ZALiA: music volume 0-10.",
        );
        slider_u8(
            ui,
            &mut d.sfx_volume,
            0..=MAX_VOLUME,
            "Sound effects volume",
            "10 = unchanged. ZALiA: sound volume 0-10.",
        );
        check(
            ui,
            &mut d.low_hp_beep_reduced,
            "Quieter low-health beep",
            "The low-health beep plays less often after a few repeats. ZALiA: \
             mod_LOW_HP_SOUND.",
        );
    });
}

fn dev_group(ui: &mut egui::Ui, d: &mut DisplayEnh, o: Opts) {
    group(ui, "Dev tools", "enh-dev", o.force_open, true, |ui| {
        check(
            ui,
            &mut d.dev_hitboxes,
            "Show hitboxes",
            "Draw the collision boxes. ZALiA: Dev tools > Hitboxes.",
        );
        check(
            ui,
            &mut d.dev_xy,
            "Show XY points",
            "Draw object positions. ZALiA: Dev tools > XY points.",
        );
        check(
            ui,
            &mut d.dev_hp,
            "Show enemy HP",
            "Draw enemy hit points. ZALiA: Dev tools > HP.",
        );
        check(
            ui,
            &mut d.dev_framecount,
            "Show frame count",
            "Draw the frame counter. ZALiA: Dev tools > App frame count.",
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every group renders headless, editable and read-only, with the
    /// original options and the ZALiA preset.
    #[test]
    fn every_group_renders_headless() {
        let ctx = egui::Context::default();
        for editable in [true, false] {
            for zalia in [false, true] {
                let (mut e, mut d) = if zalia {
                    (Enhancements::zalia_preset(), DisplayEnh::zalia_preset())
                } else {
                    (Enhancements::default(), DisplayEnh::default())
                };
                let o = Opts {
                    force_open: true,
                    gameplay_editable: editable,
                    rom_randomized: zalia,
                };
                let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                    enhancements_section(ui, &mut e, &mut d, o);
                });
                out.textures_delta.clear();
                // Drawing alone never edits anything.
                if !zalia {
                    assert_eq!(e, Enhancements::default());
                    assert_eq!(d, DisplayEnh::default());
                }
            }
        }
    }

    #[test]
    fn only_three_groups_are_display_only() {
        let display: Vec<Group> = Group::ALL
            .into_iter()
            .filter(|g| !g.is_gameplay())
            .collect();
        assert_eq!(display, [Group::Graphics, Group::Audio, Group::Dev]);
    }
}
