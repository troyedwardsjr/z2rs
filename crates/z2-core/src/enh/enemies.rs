//! Enemy and boss changes (group `E`).
//!
//! Options ([`EnemyOpts`]) and their ZALiA reference behaviour. Every
//! routine named below lives in a switchable PRG bank (`$8000-$BFFF`), so
//! each hook body first checks that the expected bank is mapped and
//! otherwise runs whatever the hook replaced ([`call_original`]); trap keys
//! alias across banks (see `traps.rs`, "Bank aliasing caveat").
//!
//! * `ironknuckle_aggro` (ZALiA `mod_IronKnuckle_AggroAI=3`, `AggroAI2`):
//!   the aggro test of `bank4_Enemy_Routines_Iron_Knuckle` (bank 4
//!   `$9D38-$9D51`: "outside the window → idle", then "Link 0..`$29` px
//!   above → aggro") is replaced in the in-memory PRG by
//!   `JSR $9D40 : BCC $9D52 : JMP $9DEE`, and `$9D40` (now unreachable
//!   filler) is trapped by [`ik_aggro`]. Iron Knuckles keep aggro while Link
//!   is up to [`IK_ABOVE_RANGE`] px above (a full jump; the original drops it
//!   at `$2A`) and still never while Link is below them. An Iron Knuckle
//!   outside the original 256-px picture (its off-screen bits `$C9 & $0C`)
//!   aggros only when it stands in the visible widescreen margin (wide
//!   gameplay on) **and** faces Link; without widescreen that is never,
//!   exactly as in the original.
//! * `ra_hp_reduced` (ZALiA `mod_Ra_HP`): Ra's entries in the per-world
//!   enemy HP tables that the area loader copies to WRAM `$6D21`
//!   (`bank7_Transfer_2A1_bytes_...`, bank 7 `$CE77`) are halved in the
//!   in-memory PRG: bank 4 `$9421 + $0A` (palaces 1/2/5) and bank 5
//!   `$9421 + $16` (Great Palace). Takes effect from the next area load.
//! * `stalfos_upthrust_fix` (ZALiA `mod_STALFOS_CONTROL1`): the Stalfos
//!   routine (bank 4 `$965A`) makes itself immune to the up-thrust by writing
//!   `$F8` ("no sword") to Link's sword Y `$0480`, which also hid the sword
//!   from every enemy processed after it that frame. [`stalfos`] restores
//!   `$0480` once the Stalfos routine returns, so the immunity stays but no
//!   longer leaks.
//! * `wizard_teleport_wide` (ZALiA `mod_Wizard_TELEPORT_AREA=2`): the
//!   palace 3/4/6 wizard (world 4 code `$1D`, bank 4 `$AE4F`) teleports to
//!   page 1 only (`LAEBD`: `$3C = 1`, `$4E = rng`). [`wizard`] moves each
//!   new teleport target to a random page of the room (`$D1` = last page)
//!   and an X kept [`TELEPORT_PAD`] px off the page edges, from a per-slot
//!   hash so several wizards spread out.
//! * `mago_balance` (ZALiA Mago `ADJ1`, `ADJ3`): Mago (world 3 code `$1D`,
//!   bank 4 `$B7C5`) spawned its fireball 8 px *outside* its left edge when
//!   facing left but 8 px *inside* when facing right (X offset tables
//!   `$B7BE`/`$B7C0`, indexed by facing 1 = right, 2 = left). The left
//!   entries (`$B7C0`, `$B7C2`) become `+0` so both sides mirror. A Mago that
//!   is alone in the room teleports to [`MAGO_NEAR_MIN`]..+63 px from Link
//!   ([`mago`]) instead of anywhere on screen.
//! * `boss_first_attack_delay`: Horsehead (`$BB5F`), Helmethead and Gooma
//!   (`$BAC3`) and Carock (`$AE7B`) cannot start an attack for
//!   [`BOSS_DELAY_FRAMES`] once the fight starts (the boss routine locks the
//!   arena scroll, `$0728 != 0`; Helmethead fires on that very frame in the
//!   original): projectiles they spawn are dropped and a melee swing that
//!   starts (`$81` leaving 0, Horsehead and Gooma) is held back. Uses
//!   `EnhState::timers[3]`: bit 15 = armed for the current boss, bits 0-14 =
//!   frames left (counted down in `end_of_frame`); cleared when no boss is
//!   present.
//! * `helmethead_fix` (ZALiA `Vulnerability=1`): **partial**. Helmethead
//!   (world 3, region 0, `$BAC3`) cannot be hurt by the sword while a lost
//!   head is regrowing (`$81 != 0` and the regrow cooldown `$05DE != 0`):
//!   the hook hides the sword (`$0480 = $F8`) for the boss routine only. The
//!   Fenser height part (`mod_FenserFix1`) is not implemented: the floating
//!   head's hover band (`$BD24-$BD58`) has no evident "correct" target to
//!   restore without the ZALiA reference values.
//! * `carock_longer_vuln` (ZALiA `VulnDur=1`): Carock (world 4 code `$22`,
//!   bank 4 `$AE7B`) is solid for 16 frames per appearance (`$AF` in
//!   `$E0-$EF`). The two thresholds (`CMP #$D0` at `$AEA6`, `CMP #$E0` at
//!   `$AEAA`) become `$C0`/`$D0`: 32 visible frames, the vanish flicker and
//!   the beam timing unchanged.
//! * `barba_aim` (ZALiA Barba `Aim=1`): Barba (world 4 code `$21`, bank 4
//!   `$B11F`) fires with a velocity picked from the horizontal distance only,
//!   then moves the shot to its mouth. [`barba`] re-aims each new shot from
//!   where it actually starts towards Link's centre, keeping the original
//!   speed.
//! * `p5_horsehead` (ZALiA `mod_P5HorseHead`): **not implemented** (no-op).
//!   Placing Horsehead (a boss: `LBE8B` freezes scrolling and starts the
//!   boss music, `LC2A6` keys it to the room's item-presence bit, its death
//!   runs the boss key/elevator sequence) into a regular multi-page room
//!   needs more than an enemy-table byte to stay safe.

use serde::{Deserialize, Serialize};

use super::{call_original, hook, patch_prg_at};
use crate::cpu::FLAG_C;
use crate::game::Game;

/// Enemy and boss options. `Default` is the original game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EnemyOpts {
    /// Iron Knuckle aggro rules.
    pub ironknuckle_aggro: bool,
    /// Ra with less HP.
    pub ra_hp_reduced: bool,
    /// Stalfos upthrust collision fix.
    pub stalfos_upthrust_fix: bool,
    /// Wizards teleport across the whole map.
    pub wizard_teleport_wide: bool,
    /// Mago balance.
    pub mago_balance: bool,
    /// Bosses wait before attacking.
    pub boss_first_attack_delay: bool,
    /// Helmethead head height fix.
    pub helmethead_fix: bool,
    /// Carock vulnerable longer.
    pub carock_longer_vuln: bool,
    /// Barba aims.
    pub barba_aim: bool,
    /// Horsehead in the P5 false-wall room.
    pub p5_horsehead: bool,
}

impl EnemyOpts {
    /// Whether anything in this group is on.
    #[must_use]
    pub fn is_active(&self) -> bool {
        *self != Self::default()
    }

    /// Append this group's stable identity encoding (fixed field order).
    pub fn write_identity(&self, out: &mut Vec<u8>) {
        for b in [
            self.ironknuckle_aggro,
            self.ra_hp_reduced,
            self.stalfos_upthrust_fix,
            self.wizard_teleport_wide,
            self.mago_balance,
            self.boss_first_attack_delay,
            self.helmethead_fix,
            self.carock_longer_vuln,
            self.barba_aim,
            self.p5_horsehead,
        ] {
            out.push(u8::from(b));
        }
    }
}

// ------------------------------------------------------------ constants

/// PRG bank holding the palace 1-6 enemy code.
const BANK_PALACE: u8 = 4;
/// PRG bank holding the Great Palace enemy data.
const BANK_GREAT_PALACE: u8 = 5;

/// `$0707` world: palaces 1/2/5 (bank 4, tables at `$9400`).
const WORLD_PALACE_A: u8 = 3;
/// `$0707` world: palaces 3/4/6 (bank 4, tables at `$A900`).
const WORLD_PALACE_B: u8 = 4;

/// `bank4_Enemy_Routines_Stalfos`.
pub const ADDR_STALFOS: u16 = 0x965A;
/// Start of the Iron Knuckle aggro test that is patched.
pub const ADDR_IK_SITE: u16 = 0x9D38;
/// Trapped filler inside the patched Iron Knuckle site.
pub const ADDR_IK_TRAMP: u16 = 0x9D40;
/// `bank4_Enemy_Routines_Mago` (world 3).
pub const ADDR_MAGO: u16 = 0xB7C5;
/// Palace 3/4/6 wizard routine (world 4 code `$1D`, `LAE4F`).
pub const ADDR_WIZARD: u16 = 0xAE4F;
/// Carock (world 4 code `$22`, `LAE7B`).
pub const ADDR_CAROCK: u16 = 0xAE7B;
/// Barba (world 4 code `$21`, `LB11F`).
pub const ADDR_BARBA: u16 = 0xB11F;
/// `bank4_Enemy_Routines_Horsehead` (world 3 code `$20`).
pub const ADDR_HORSEHEAD: u16 = 0xBB5F;
/// `bank4_Enemy_Routines_Helmethead__Gooma` (world 3 code `$21`).
pub const ADDR_HELMET_GOOMA: u16 = 0xBAC3;

/// Enemy HP table (per bank; copied to WRAM `$6D21` on area load).
const HP_TABLE: u16 = 0x9421;
/// Ra's code in palaces 1/2/5.
pub const RA_CODE_PALACE: u8 = 0x0A;
/// Ra's code in the Great Palace.
pub const RA_CODE_GREAT_PALACE: u8 = 0x16;

/// Mago fireball X offset, low byte, facing left (`$B7BE + 2`).
const MAGO_LEFT_LO: u16 = 0xB7C0;
/// Mago fireball X offset, high byte, facing left (`$B7C0 + 2`).
const MAGO_LEFT_HI: u16 = 0xB7C2;
/// Mago's code (world 3).
const MAGO_CODE: u8 = 0x1D;

/// Carock thresholds: `CMP #$D0` (vanish below) and `CMP #$E0` (solid at
/// and above).
const CAROCK_VANISH_CMP: u16 = 0xAEA6;
const CAROCK_SOLID_CMP: u16 = 0xAEAA;

/// Iron Knuckle: most px Link may be above it and keep its aggro (the
/// original stops at `$2A`; a full jump reaches about `$30`).
pub const IK_ABOVE_RANGE: u8 = 0x34;
/// Original Iron Knuckle aggro height.
const IK_ABOVE_RANGE_OG: u8 = 0x2A;

/// Wizard teleport: px kept clear of each page edge.
pub const TELEPORT_PAD: u8 = 16;
/// Lone Mago: nearest teleport distance from Link (px).
pub const MAGO_NEAR_MIN: u16 = 40;

/// Frames a boss holds its first attack.
pub const BOSS_DELAY_FRAMES: u16 = 120;
/// `timers[3]` bit 15: the delay was armed for the boss now present.
const BOSS_ARMED: u16 = 0x8000;

// RAM (enemy slot arrays are `base + slot`).
const SLOT: usize = 0x0010;
const FRAME: usize = 0x0012;
const LINK_Y: usize = 0x0029;
const ENEMY_Y: usize = 0x002A;
const PROJ_Y: usize = 0x0030;
const LINK_PAGE: usize = 0x003B;
const ENEMY_PAGE: usize = 0x003C;
const PROJ_PAGE: usize = 0x0042;
const LINK_X: usize = 0x004D;
const ENEMY_X: usize = 0x004E;
const PROJ_X: usize = 0x0054;
const ENEMY_FACING: usize = 0x0060;
const PROJ_FACING: usize = 0x0066;
const PROJ_XVEL: usize = 0x0077;
const ENEMY_ANIM: usize = 0x0081;
const PROJ_TYPE: usize = 0x0087;
const ENEMY_CODE: usize = 0x00A1;
const ENEMY_AUX: usize = 0x00AF;
const ENEMY_SLOT_ON: usize = 0x00B6;
const OFFSCREEN: usize = 0x00C9;
const AREA_LAST_PAGE: usize = 0x00D1;
const SWORD_Y: usize = 0x0480;
const ENEMY_TIMER: usize = 0x0504;
const RNG: usize = 0x051B;
const PROJ_YVEL: usize = 0x0584;
const REGROW: usize = 0x05DE;
const REGION: usize = 0x0706;
const WORLD: usize = 0x0707;
const FREEZE_SCROLL: usize = 0x0728;
const SCROLL_HI: usize = 0x072A;
const SCROLL_LO: usize = 0x072C;

const SLOTS: usize = 6;
/// `$0480` value meaning "no sword this frame".
const SWORD_OFF: u8 = 0xF8;

// ------------------------------------------------------------ install

/// Install this group's hooks ([`super::apply`], only when active).
pub(crate) fn register(game: &mut Game, opts: &EnemyOpts) {
    if opts.ironknuckle_aggro {
        install_ik(game);
    }
    if opts.ra_hp_reduced {
        halve_hp(game, BANK_PALACE, RA_CODE_PALACE);
        halve_hp(game, BANK_GREAT_PALACE, RA_CODE_GREAT_PALACE);
    }
    if opts.stalfos_upthrust_fix {
        hook(
            game,
            "enh_enemies_stalfos",
            Some(BANK_PALACE),
            ADDR_STALFOS,
            stalfos,
            Some(0),
        );
    }
    if opts.wizard_teleport_wide {
        hook(
            game,
            "enh_enemies_wizard",
            Some(BANK_PALACE),
            ADDR_WIZARD,
            wizard,
            Some(0),
        );
    }
    if opts.mago_balance {
        // `ADC LB7BE,x` / `ADC LB7C0,x` must be where the tables are read.
        if prg_matches(game, BANK_PALACE, 0xB7ED, &[0x7D, 0xBE, 0xB7])
            && prg_matches(game, BANK_PALACE, 0xB7F6, &[0x7D, 0xC0, 0xB7])
        {
            patch_prg_at(game, BANK_PALACE, MAGO_LEFT_LO, &[0x00]);
            patch_prg_at(game, BANK_PALACE, MAGO_LEFT_HI, &[0x00]);
        }
        hook(
            game,
            "enh_enemies_mago",
            Some(BANK_PALACE),
            ADDR_MAGO,
            mago,
            Some(0),
        );
    }
    if opts.boss_first_attack_delay {
        hook(
            game,
            "enh_enemies_horsehead",
            Some(BANK_PALACE),
            ADDR_HORSEHEAD,
            horsehead,
            Some(0),
        );
    }
    if opts.boss_first_attack_delay || opts.helmethead_fix {
        hook(
            game,
            "enh_enemies_helmet_gooma",
            Some(BANK_PALACE),
            ADDR_HELMET_GOOMA,
            helmet_gooma,
            Some(0),
        );
    }
    if opts.carock_longer_vuln
        && prg_matches(game, BANK_PALACE, CAROCK_VANISH_CMP, &[0xC9, 0xD0])
        && prg_matches(game, BANK_PALACE, CAROCK_SOLID_CMP, &[0xC9, 0xE0])
    {
        patch_prg_at(game, BANK_PALACE, CAROCK_VANISH_CMP + 1, &[0xC0]);
        patch_prg_at(game, BANK_PALACE, CAROCK_SOLID_CMP + 1, &[0xD0]);
    }
    if opts.boss_first_attack_delay {
        hook(
            game,
            "enh_enemies_carock",
            Some(BANK_PALACE),
            ADDR_CAROCK,
            carock,
            Some(0),
        );
    }
    if opts.barba_aim {
        hook(
            game,
            "enh_enemies_barba",
            Some(BANK_PALACE),
            ADDR_BARBA,
            barba,
            Some(0),
        );
    }
    // p5_horsehead: documented no-op (see the module docs).
}

/// Per-frame work after [`Game::step`] (only while any enhancement is on).
pub(crate) fn end_of_frame(game: &mut Game, opts: &EnemyOpts) {
    if opts.boss_first_attack_delay {
        let t = game.enh_state.timers[3];
        game.enh_state.timers[3] = if !boss_present(game) {
            0
        } else if t & !BOSS_ARMED != 0 {
            t - 1
        } else {
            t
        };
    }
}

// ------------------------------------------------------------ helpers

/// PRG image offset of `addr` in 16 KiB `bank`.
fn prg_offset(bank: u8, addr: u16) -> usize {
    usize::from(bank) * 0x4000 + usize::from(addr & 0x3FFF)
}

/// Whether the in-memory PRG holds `bytes` at `bank:addr`.
fn prg_matches(game: &Game, bank: u8, addr: u16, bytes: &[u8]) -> bool {
    let o = prg_offset(bank, addr);
    game.prg.get(o..o + bytes.len()) == Some(bytes)
}

/// Whether switchable `bank` is mapped at `$8000-$BFFF`.
fn bank_mapped(game: &Game, bank: u8) -> bool {
    game.prg.len() >= 0x8000
        && game.mmc1.map_prg(0x8000, game.prg.len()) == usize::from(bank) * 0x4000
}

/// Bank 4 mapped and the world one of `worlds`.
fn in_palace(game: &Game, worlds: &[u8]) -> bool {
    bank_mapped(game, BANK_PALACE) && worlds.contains(&game.ram[WORLD])
}

/// Current enemy slot (`$10`).
fn slot(game: &Game) -> usize {
    usize::from(game.ram[SLOT]) % SLOTS
}

/// Absolute room X of enemy slot `s`.
fn enemy_abs_x(game: &Game, s: usize) -> u16 {
    u16::from_be_bytes([game.ram[ENEMY_PAGE + s], game.ram[ENEMY_X + s]])
}

/// Absolute room X of Link.
fn link_abs_x(game: &Game) -> u16 {
    u16::from_be_bytes([game.ram[LINK_PAGE], game.ram[LINK_X]])
}

/// Absolute room X of the left edge of the 256-px picture.
fn scroll_abs_x(game: &Game) -> u16 {
    u16::from_be_bytes([game.ram[SCROLL_HI], game.ram[SCROLL_LO]])
}

/// Deterministic per-slot hash for teleport targets (depends only on game
/// state, so every netplay peer computes the same value).
fn slot_hash(game: &Game, s: usize, salt: u32) -> u32 {
    let mut h = u32::from(game.ram[RNG + s])
        | (u32::from(game.ram[FRAME]) << 8)
        | ((s as u32) << 16)
        | (salt << 20);
    h ^= (game.enh_state.frames as u32).wrapping_mul(0x9E37_79B1);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^ (h >> 15)
}

/// The projectile types (`$87`) of the six projectile slots.
fn proj_types(game: &Game) -> [u8; SLOTS] {
    let mut t = [0u8; SLOTS];
    t.copy_from_slice(&game.ram[PROJ_TYPE..PROJ_TYPE + SLOTS]);
    t
}

/// Integer square root (floor).
fn isqrt(v: u32) -> u32 {
    if v < 2 {
        return v;
    }
    let mut x = v;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + v / x) / 2;
    }
    x
}

// ------------------------------------------------------------ Ra HP

/// Halve enemy `code`'s entry in `bank`'s HP table (`0` and `$FF` are left
/// alone: "none" and "invincible").
fn halve_hp(game: &mut Game, bank: u8, code: u8) {
    let o = prg_offset(bank, HP_TABLE + u16::from(code));
    let Some(&hp) = game.prg.get(o) else {
        return;
    };
    if hp != 0 && hp != 0xFF {
        patch_prg_at(game, bank, HP_TABLE + u16::from(code), &[reduced_hp(hp)]);
    }
}

/// Ra's reduced HP: half, at least 1.
#[must_use]
pub const fn reduced_hp(hp: u8) -> u8 {
    let h = hp / 2;
    if h == 0 {
        1
    } else {
        h
    }
}

// ------------------------------------------------------------ Iron Knuckle

/// Patch the Iron Knuckle aggro test into `JSR tramp : BCC aggro : JMP idle`
/// and trap the trampoline. Skipped when the bytes are not the expected
/// routine (a different ROM revision).
fn install_ik(game: &mut Game) {
    // `LDA $A1,x` opens the test, `JMP L9DEE` closes it.
    if !prg_matches(game, BANK_PALACE, ADDR_IK_SITE, &[0xB5, 0xA1])
        || !prg_matches(game, BANK_PALACE, 0x9D4F, &[0x4C, 0xEE, 0x9D])
    {
        return;
    }
    let [lo, hi] = ADDR_IK_TRAMP.to_le_bytes();
    let mut bytes = [0xEAu8; 0x1A]; // $9D38-$9D51
    bytes[..8].copy_from_slice(&[
        0x20, lo, hi, // JSR $9D40
        0x90, 0x15, // BCC $9D52 (aggro)
        0x4C, 0xEE, 0x9D, // JMP $9DEE (idle)
    ]);
    patch_prg_at(game, BANK_PALACE, ADDR_IK_SITE, &bytes);
    hook(
        game,
        "enh_enemies_ik_aggro",
        Some(BANK_PALACE),
        ADDR_IK_TRAMP,
        ik_aggro,
        Some(40),
    );
}

/// Inputs of the Iron Knuckle aggro decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IkView {
    /// Enemy code (`$A1,x`).
    pub code: u8,
    /// Off-screen bits (`$C9`).
    pub offscreen: u8,
    /// Iron Knuckle Y (`$2A,x`).
    pub ik_y: u8,
    /// Link Y (`$29`).
    pub link_y: u8,
    /// Iron Knuckle faces Link.
    pub facing_link: bool,
    /// Iron Knuckle stands in the visible widescreen margin.
    pub in_margin: bool,
}

/// The original aggro test (`$9D38-$9D51`): `true` = aggro.
#[must_use]
pub const fn ik_aggro_og(v: IkView) -> bool {
    if v.code >= 0x18 && v.offscreen & 0x0C != 0 {
        return false;
    }
    match v.ik_y.checked_sub(v.link_y) {
        Some(d) => d < IK_ABOVE_RANGE_OG,
        None => false,
    }
}

/// The enhanced aggro test (see the module docs).
#[must_use]
pub const fn ik_aggro_enh(v: IkView) -> bool {
    let in_range = match v.ik_y.checked_sub(v.link_y) {
        Some(d) => d < IK_ABOVE_RANGE,
        None => false,
    };
    if !in_range {
        return false;
    }
    if v.code >= 0x18 && v.offscreen & 0x0C != 0 {
        return v.in_margin && v.facing_link;
    }
    true
}

/// Trampoline at `$9D40`: `C` clear = aggro, set = idle.
fn ik_aggro(game: &mut Game) {
    if !bank_mapped(game, BANK_PALACE) {
        call_original(game, ADDR_IK_TRAMP);
        return;
    }
    let s = slot(game);
    let ik = enemy_abs_x(game, s);
    let link = link_abs_x(game);
    // Facing: 1 = right, 2 = left.
    let facing_right = game.ram[ENEMY_FACING + s] == 1;
    let in_margin = game.wide_game.enabled && {
        let m = i32::from(game.wide_game.margin_px);
        let sx = i32::from(ik) - i32::from(scroll_abs_x(game));
        sx >= -m && sx < 256 + m
    };
    let v = IkView {
        code: game.ram[ENEMY_CODE + s],
        offscreen: game.ram[OFFSCREEN],
        ik_y: game.ram[ENEMY_Y + s],
        link_y: game.ram[LINK_Y],
        facing_link: (link >= ik) == facing_right,
        in_margin,
    };
    let aggro = if game.enh.enemies.ironknuckle_aggro {
        ik_aggro_enh(v)
    } else {
        ik_aggro_og(v)
    };
    if aggro {
        game.cpu.p &= !FLAG_C;
    } else {
        game.cpu.p |= FLAG_C;
    }
}

// ------------------------------------------------------------ Stalfos

/// `$965A` wrapper: keep the Stalfos' up-thrust immunity local to it.
fn stalfos(game: &mut Game) {
    if !in_palace(game, &[WORLD_PALACE_A, WORLD_PALACE_B]) || !game.enh.enemies.stalfos_upthrust_fix
    {
        call_original(game, ADDR_STALFOS);
        return;
    }
    let sword_y = game.ram[SWORD_Y];
    call_original(game, ADDR_STALFOS);
    if sword_y != SWORD_OFF && game.ram[SWORD_Y] == SWORD_OFF {
        game.ram[SWORD_Y] = sword_y;
    }
}

// ------------------------------------------------------------ wizards

/// Wide wizard teleport target: page `0..=last_page`, X padded from both
/// page edges. Returns `(page, x)`.
#[must_use]
pub fn wizard_target(hash: u32, last_page: u8) -> (u8, u8) {
    let pages = u32::from(last_page.min(3)) + 1;
    let page = (hash % pages) as u8;
    let span = 256 - 2 * u32::from(TELEPORT_PAD) - 16;
    let x = TELEPORT_PAD + ((hash >> 8) % span) as u8;
    (page, x)
}

/// `$AE4F` wrapper (palace 3/4/6 wizard): spread teleports over the room.
fn wizard(game: &mut Game) {
    if !in_palace(game, &[WORLD_PALACE_B]) || !game.enh.enemies.wizard_teleport_wide {
        call_original(game, ADDR_WIZARD);
        return;
    }
    let s = slot(game);
    let aux = game.ram[ENEMY_AUX + s];
    call_original(game, ADDR_WIZARD);
    // `LAEBD` ran (teleport) when $AF dropped below $40 and was reset to 0.
    if (1..=0x40).contains(&aux) && game.ram[ENEMY_AUX + s] == 0 {
        let h = slot_hash(game, s, 1);
        let (page, x) = wizard_target(h, game.ram[AREA_LAST_PAGE]);
        game.ram[ENEMY_PAGE + s] = page;
        game.ram[ENEMY_X + s] = x;
    }
}

/// Lone-Mago teleport target near Link, kept inside the picture
/// (`scroll`..`scroll + 240`). Returns the absolute X.
#[must_use]
pub fn mago_near_target(hash: u32, link: u16, scroll: u16) -> u16 {
    let off = MAGO_NEAR_MIN + (hash & 0x3F) as u16;
    let lo = scroll.saturating_add(8);
    let hi = scroll.saturating_add(232);
    let right = link.saturating_add(off).clamp(lo, hi);
    let left = link.saturating_sub(off).clamp(lo, hi);
    let (first, second) = if hash & 0x100 != 0 {
        (right, left)
    } else {
        (left, right)
    };
    // Prefer the side that keeps the full distance after clamping.
    if first.abs_diff(link) >= MAGO_NEAR_MIN / 2 {
        first
    } else {
        second
    }
}

/// `$B7C5` wrapper (Mago, world 3): a lone Mago teleports near Link.
fn mago(game: &mut Game) {
    if !in_palace(game, &[WORLD_PALACE_A]) || !game.enh.enemies.mago_balance {
        call_original(game, ADDR_MAGO);
        return;
    }
    let s = slot(game);
    call_original(game, ADDR_MAGO);
    // `LB81E` (teleport, every invisible frame) runs while $AF < $A0.
    if game.ram[ENEMY_AUX + s] >= 0xA0 {
        return;
    }
    let lone = (0..SLOTS).all(|o| {
        o == s || game.ram[ENEMY_SLOT_ON + o] == 0 || game.ram[ENEMY_CODE + o] != MAGO_CODE
    });
    if !lone {
        return;
    }
    let h = slot_hash(game, s, 2);
    let x = mago_near_target(h, link_abs_x(game), scroll_abs_x(game));
    let [page, lo] = x.to_be_bytes();
    game.ram[ENEMY_PAGE + s] = page;
    game.ram[ENEMY_X + s] = lo;
}

// ------------------------------------------------------------ bosses

/// A boss of the delay list occupies a slot.
fn boss_present(game: &Game) -> bool {
    let codes: &[u8] = match game.ram[WORLD] {
        WORLD_PALACE_A => &[0x20, 0x21],
        WORLD_PALACE_B => &[0x22],
        _ => return false,
    };
    (0..SLOTS)
        .any(|s| game.ram[ENEMY_SLOT_ON + s] != 0 && codes.contains(&game.ram[ENEMY_CODE + s]))
}

/// Run a boss routine at `addr`, holding back its attacks while the first
/// attack delay runs. `melee`: a swing starting (`$81` leaving 0) counts as
/// an attack too.
fn boss_with_delay(game: &mut Game, addr: u16, melee: bool) {
    if !game.enh.enemies.boss_first_attack_delay {
        call_original(game, addr);
        return;
    }
    let s = slot(game);
    let before = proj_types(game);
    let anim = game.ram[ENEMY_ANIM + s];
    let timer = game.ram[ENEMY_TIMER + s];
    call_original(game, addr);
    // Arm once the fight is on: the arena locks the scroll (`$0728`), which
    // the boss routine itself does on the frame Link reaches the arena (and
    // may fire on that very frame).
    if game.enh_state.timers[3] == 0 && game.ram[FREEZE_SCROLL] != 0 {
        game.enh_state.timers[3] = BOSS_ARMED | BOSS_DELAY_FRAMES;
    }
    if game.enh_state.timers[3] & !BOSS_ARMED == 0 {
        return;
    }
    for (p, &t) in before.iter().enumerate() {
        if t == 0 {
            game.ram[PROJ_TYPE + p] = 0;
        }
    }
    if melee && anim == 0 && game.ram[ENEMY_ANIM + s] != 0 {
        game.ram[ENEMY_ANIM + s] = 0;
        game.ram[ENEMY_TIMER + s] = timer;
    }
}

/// `$BB5F` wrapper (Horsehead).
fn horsehead(game: &mut Game) {
    if !in_palace(game, &[WORLD_PALACE_A]) {
        call_original(game, ADDR_HORSEHEAD);
        return;
    }
    boss_with_delay(game, ADDR_HORSEHEAD, true);
}

/// `$BAC3` wrapper (Helmethead in region 0, Gooma elsewhere).
fn helmet_gooma(game: &mut Game) {
    if !in_palace(game, &[WORLD_PALACE_A]) {
        call_original(game, ADDR_HELMET_GOOMA);
        return;
    }
    let s = slot(game);
    let helmethead = game.ram[REGION] == 0;
    let shield = helmethead
        && game.enh.enemies.helmethead_fix
        && game.ram[ENEMY_ANIM + s] != 0
        && game.ram[REGROW] != 0
        && game.ram[SWORD_Y] != SWORD_OFF;
    let sword_y = game.ram[SWORD_Y];
    if shield {
        game.ram[SWORD_Y] = SWORD_OFF;
    }
    boss_with_delay(game, ADDR_HELMET_GOOMA, !helmethead);
    if shield && game.ram[SWORD_Y] == SWORD_OFF {
        game.ram[SWORD_Y] = sword_y;
    }
}

/// `$AE7B` wrapper (Carock).
fn carock(game: &mut Game) {
    if !in_palace(game, &[WORLD_PALACE_B]) {
        call_original(game, ADDR_CAROCK);
        return;
    }
    boss_with_delay(game, ADDR_CAROCK, false);
}

/// Velocity `(vx, vy)` of speed `|(vx0, vy0)|` from `(px, py)` towards
/// `(tx, ty)`, each clamped to a signed byte.
#[must_use]
pub fn aim_velocity(vx0: i8, vy0: i8, px: i32, py: i32, tx: i32, ty: i32) -> (i8, i8) {
    let speed2 = i32::from(vx0).pow(2) + i32::from(vy0).pow(2);
    let speed = isqrt(speed2.unsigned_abs()) as i32;
    let (dx, dy) = (tx - px, ty - py);
    let dist = isqrt((dx * dx + dy * dy).unsigned_abs()) as i32;
    if speed == 0 || dist == 0 {
        return (vx0, vy0);
    }
    // n * speed / dist, rounded half away from zero.
    let round = |n: i32| -> i8 {
        let a = n.abs() * speed;
        let q = (2 * a + dist) / (2 * dist);
        (q * n.signum()).clamp(-127, 127) as i8
    };
    (round(dx), round(dy))
}

/// `$B11F` wrapper (Barba): aim new fireballs at Link.
fn barba(game: &mut Game) {
    if !in_palace(game, &[WORLD_PALACE_B]) || !game.enh.enemies.barba_aim {
        call_original(game, ADDR_BARBA);
        return;
    }
    let before = proj_types(game);
    call_original(game, ADDR_BARBA);
    for (p, &t) in before.iter().enumerate() {
        if t != 0 || game.ram[PROJ_TYPE + p] == 0 {
            continue;
        }
        let px = i32::from(u16::from_be_bytes([
            game.ram[PROJ_PAGE + p],
            game.ram[PROJ_X + p],
        ])) + 4;
        let py = i32::from(game.ram[PROJ_Y + p]) + 4;
        let tx = i32::from(link_abs_x(game)) + 8;
        let ty = i32::from(game.ram[LINK_Y]) + 16;
        let (vx, vy) = aim_velocity(
            game.ram[PROJ_XVEL + p] as i8,
            game.ram[PROJ_YVEL + p] as i8,
            px,
            py,
            tx,
            ty,
        );
        game.ram[PROJ_XVEL + p] = vx as u8;
        game.ram[PROJ_YVEL + p] = vy as u8;
        if vx != 0 {
            game.ram[PROJ_FACING + p] = if vx > 0 { 1 } else { 2 };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enh::Enhancements;

    fn view() -> IkView {
        IkView {
            code: 0x18,
            offscreen: 0,
            ik_y: 0x90,
            link_y: 0x90,
            facing_link: true,
            in_margin: false,
        }
    }

    #[test]
    fn ik_og_matches_the_rom_test() {
        let v = view();
        assert!(ik_aggro_og(v));
        assert!(ik_aggro_og(IkView {
            link_y: 0x90 - 0x29,
            ..v
        }));
        assert!(!ik_aggro_og(IkView {
            link_y: 0x90 - 0x2A,
            ..v
        }));
        assert!(!ik_aggro_og(IkView { link_y: 0x91, ..v }));
        assert!(!ik_aggro_og(IkView {
            offscreen: 0x04,
            ..v
        }));
    }

    #[test]
    fn ik_enhanced_range_facing_and_margin() {
        let v = view();
        // Higher above: still aggro, up to the new range.
        assert!(ik_aggro_enh(IkView {
            link_y: 0x90 - 0x30,
            ..v
        }));
        assert!(!ik_aggro_enh(IkView {
            link_y: 0x90 - IK_ABOVE_RANGE,
            ..v
        }));
        // Below: never.
        assert!(!ik_aggro_enh(IkView { link_y: 0x91, ..v }));
        // Off the original picture: only in the margin and facing Link.
        let off = IkView {
            offscreen: 0x08,
            ..v
        };
        assert!(!ik_aggro_enh(off));
        assert!(!ik_aggro_enh(IkView {
            in_margin: true,
            facing_link: false,
            ..off
        }));
        assert!(ik_aggro_enh(IkView {
            in_margin: true,
            ..off
        }));
    }

    #[test]
    fn ra_hp_halves_and_never_reaches_zero() {
        assert_eq!(reduced_hp(0x38), 0x1C);
        assert_eq!(reduced_hp(0x60), 0x30);
        assert_eq!(reduced_hp(1), 1);
    }

    #[test]
    fn wizard_targets_stay_padded_and_cover_every_page() {
        let mut pages = [false; 4];
        for h in 0..4096u32 {
            let hh = h.wrapping_mul(0x9E37_79B1);
            let (p, x) = wizard_target(hh, 3);
            assert!(p <= 3);
            pages[usize::from(p)] = true;
            assert!(x >= TELEPORT_PAD && u16::from(x) + 16 <= 256 - u16::from(TELEPORT_PAD));
            assert_eq!(wizard_target(hh, 0).0, 0);
        }
        assert!(pages.iter().all(|&p| p));
    }

    #[test]
    fn lone_mago_lands_near_link_inside_the_picture() {
        for h in 0..2048u32 {
            for &(link, scroll) in &[(0x180u16, 0x100u16), (0x108, 0x100), (0x1F0, 0x100)] {
                let x = mago_near_target(h, link, scroll);
                assert!(x >= scroll + 8 && x <= scroll + 232, "{h} {link:#x} {x:#x}");
                assert!(
                    x.abs_diff(link) >= MAGO_NEAR_MIN / 2,
                    "{h} {link:#x} {x:#x}"
                );
                assert!(x.abs_diff(link) < MAGO_NEAR_MIN + 64);
            }
        }
    }

    #[test]
    fn aim_keeps_speed_and_points_at_the_target() {
        // Straight right at speed 32 → straight down.
        assert_eq!(aim_velocity(32, 0, 0, 0, 0, 100), (0, 32));
        assert_eq!(aim_velocity(32, 0, 0, 0, -100, 0), (-32, 0));
        let (vx, vy) = aim_velocity(-24, 16, 100, 50, 20, 110);
        assert!(vx < 0 && vy > 0);
        let s0 = 24 * 24 + 16 * 16;
        let s1 = i32::from(vx).pow(2) + i32::from(vy).pow(2);
        assert!((s1 - s0).abs() <= 2 * 29 + 2, "{s0} {s1}");
        // Degenerate inputs keep the original velocity.
        assert_eq!(aim_velocity(5, -3, 7, 7, 7, 7), (5, -3));
        assert_eq!(aim_velocity(0, 0, 0, 0, 9, 9), (0, 0));
        assert_eq!(isqrt(0), 0);
        assert_eq!(isqrt(99), 9);
        assert_eq!(isqrt(100), 10);
    }

    #[test]
    fn boss_delay_timer_arms_counts_down_and_resets() {
        let mut g = Game::new();
        let mut e = Enhancements::default();
        e.enemies.boss_first_attack_delay = true;
        g.set_enhancements(e);
        // No boss: stays idle.
        g.ram[WORLD] = WORLD_PALACE_A;
        end_of_frame(&mut g, &e.enemies);
        assert_eq!(g.enh_state.timers[3], 0);
        // Boss present and armed: counts down to the armed bit, then holds.
        g.ram[ENEMY_SLOT_ON + 2] = 1;
        g.ram[ENEMY_CODE + 2] = 0x20;
        g.enh_state.timers[3] = BOSS_ARMED | 2;
        end_of_frame(&mut g, &e.enemies);
        end_of_frame(&mut g, &e.enemies);
        assert_eq!(g.enh_state.timers[3], BOSS_ARMED);
        end_of_frame(&mut g, &e.enemies);
        assert_eq!(g.enh_state.timers[3], BOSS_ARMED);
        // Boss gone: disarmed for the next one.
        g.ram[ENEMY_SLOT_ON + 2] = 0;
        end_of_frame(&mut g, &e.enemies);
        assert_eq!(g.enh_state.timers[3], 0);
        // Code $22 is a boss only in world 4 (Carock).
        g.ram[ENEMY_SLOT_ON] = 1;
        g.ram[ENEMY_CODE] = 0x22;
        assert!(!boss_present(&g));
        g.ram[WORLD] = WORLD_PALACE_B;
        assert!(boss_present(&g));
    }

    #[test]
    fn hooks_install_per_option_and_clear() {
        let mut g = Game::new();
        let traps = g.traps.len();
        let mut e = Enhancements::default();
        e.enemies.stalfos_upthrust_fix = true;
        e.enemies.wizard_teleport_wide = true;
        e.enemies.barba_aim = true;
        e.enemies.boss_first_attack_delay = true;
        e.enemies.p5_horsehead = true;
        g.set_enhancements(e);
        for (a, n) in [
            (ADDR_STALFOS, "enh_enemies_stalfos"),
            (ADDR_WIZARD, "enh_enemies_wizard"),
            (ADDR_BARBA, "enh_enemies_barba"),
            (ADDR_HORSEHEAD, "enh_enemies_horsehead"),
            (ADDR_HELMET_GOOMA, "enh_enemies_helmet_gooma"),
            (ADDR_CAROCK, "enh_enemies_carock"),
        ] {
            assert_eq!(g.traps.get(a).map(|t| t.name), Some(n));
        }
        g.set_enhancements(Enhancements::default());
        assert_eq!(g.traps.len(), traps);
        // Not the bank-4 IK routine (blank PRG): no patch, no trap.
        e = Enhancements::default();
        e.enemies.ironknuckle_aggro = true;
        g.set_enhancements(e);
        assert!(g.traps.get(ADDR_IK_TRAMP).is_none());
        assert_eq!(g.enh_hooks.patch_count(), 0);
    }

    #[test]
    fn slot_hash_is_deterministic_and_slot_dependent() {
        let mut g = Game::new();
        g.ram[RNG] = 7;
        g.ram[RNG + 1] = 7;
        assert_eq!(slot_hash(&g, 0, 1), slot_hash(&g, 0, 1));
        assert_ne!(slot_hash(&g, 0, 1), slot_hash(&g, 1, 1));
    }
}
