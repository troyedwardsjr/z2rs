//! Typed game-mode dispatch.
//!
//! The original threads three selector bytes through hand-rolled
//! compare/branch chains and `$D385` inline tables:
//!
//! * Game Mode `$0736` — tested at `prg7.asm $C010` (boot wait), `$C1D0`
//!   (`$0B` = sidescroll, pause path), `$D168` (change detect), `$D538`
//!   (`$0B` = sidescroll, per-frame), and set at `$C443`/`$C714`/`$CC85`/
//!   `$E187`/etc. by area/death code.
//! * Boot Stage `$076C` — values documented at `$C01B`: `00` = restart from
//!   Zelda's castle (3 lives), `01` = no routine, `02` = die, `03` = wake
//!   up Zelda, `04` = roll credits, `06` = lives screen then restart.
//!   Dispatched by `LC2CA` (`$C2CA`) through `bank7_pointer_table2`
//!   (`$C2D8`, 7 entries).
//! * Side Routine `$0524` — ranges tested in NMI (`$C157-$C162`: `< 3`,
//!   `3..7`, `>= 7`; `$C167`: `== 0`); dispatched at `$C220` through
//!   `bank7_pointer_table0` (`$C22B`: sideview handlers + save routine).
//!
//! This module replaces those jump tables with typed enums and `match`
//! dispatchers (never transpiled tables). Executing a mode body stays with
//! the owning bank (0/5/6/...); the dispatcher returns an [`Action`] naming
//! the target so tests — and later the engine — drive transitions without
//! interpreting. RAM side effects of *changing* selector (resets of
//! `$073B/$0738/$073D`/...) live in [`crate::bank7_dispatch`].

use crate::game::Game;

/// Game Mode (`$0736`). Only values observed in the bank-7 core are named;
/// everything else rides [`GameMode::Unknown`] (preserved verbatim).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameMode {
    /// `$00`: power-on / title path entry (cleared by `LD174`, `$D18A`).
    Title,
    /// `$08`: boot-wait terminal named at `$C010` (`CMP #$08`).
    BootWait8,
    /// `$0B`: sidescroll engine (`$C1D0`, `$D538`).
    SideScroll,
    /// `$14`: boot-wait terminal named at `$C010` (`CMP #$14`).
    BootWait14,
    /// Any other mode byte (area loaders own these).
    Unknown(u8),
}

impl GameMode {
    /// Decode a `$0736` byte.
    pub fn decode(v: u8) -> Self {
        match v {
            0x00 => Self::Title,
            0x08 => Self::BootWait8,
            0x0B => Self::SideScroll,
            0x14 => Self::BootWait14,
            other => Self::Unknown(other),
        }
    }

    /// Encode back to a `$0736` byte.
    pub fn encode(self) -> u8 {
        match self {
            Self::Title => 0x00,
            Self::BootWait8 => 0x08,
            Self::SideScroll => 0x0B,
            Self::BootWait14 => 0x14,
            Self::Unknown(v) => v,
        }
    }
}

/// Boot Stage (`$076C`), values documented at `prg7.asm $C01B`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootStage {
    /// `$00`: restart from Zelda's castle with 3 lives.
    RestartCastle,
    /// `$01`: no routine (steady state).
    Idle,
    /// `$02`: die.
    Die,
    /// `$03`: wake up Zelda (ending).
    WakeZelda,
    /// `$04`: roll credits.
    Credits,
    /// `$05`: table entry 5 (`$C2E0`, currently data-labelled `LC364`).
    Stage5,
    /// `$06`: lives screen, then restart the scene.
    LivesRestart,
    /// `$07+`: beyond `bank7_pointer_table2` (`$C2D8`, 7 entries).
    BeyondTable(u8),
}

impl BootStage {
    /// Decode a `$076C` byte.
    pub fn decode(v: u8) -> Self {
        match v {
            0x00 => Self::RestartCastle,
            0x01 => Self::Idle,
            0x02 => Self::Die,
            0x03 => Self::WakeZelda,
            0x04 => Self::Credits,
            0x05 => Self::Stage5,
            0x06 => Self::LivesRestart,
            other => Self::BeyondTable(other),
        }
    }

    /// Encode back to a `$076C` byte.
    pub fn encode(self) -> u8 {
        match self {
            Self::RestartCastle => 0x00,
            Self::Idle => 0x01,
            Self::Die => 0x02,
            Self::WakeZelda => 0x03,
            Self::Credits => 0x04,
            Self::Stage5 => 0x05,
            Self::LivesRestart => 0x06,
            Self::BeyondTable(v) => v,
        }
    }
}

/// Side Routine (`$0524`) ranges as tested by NMI (`$C157-$C167`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideRoutine {
    /// `$00`: overworld-ish idle (NMI `$C167` fast path).
    Idle,
    /// `$01-$02`: dialog-adjacent (`$C15C` skips when `< 3`).
    Dialog(u8),
    /// `$03-$06`: pause/pane work (`$C160` skips when `>= 7`).
    Pane(u8),
    /// `$07+`: extended routines.
    Extended(u8),
}

impl SideRoutine {
    /// Decode a `$0524` byte.
    pub fn decode(v: u8) -> Self {
        match v {
            0x00 => Self::Idle,
            0x01..=0x02 => Self::Dialog(v),
            0x03..=0x06 => Self::Pane(v),
            other => Self::Extended(other),
        }
    }

    /// Encode back to a `$0524` byte.
    pub fn encode(self) -> u8 {
        match self {
            Self::Idle => 0x00,
            Self::Dialog(v) | Self::Pane(v) | Self::Extended(v) => v,
        }
    }
}

/// What the dispatcher wants the engine to run next. Bodies live in other
/// banks (or behind `$D385` tables); the action only *names* them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Keep spinning the `$C010` boot wait.
    KeepWaiting,
    /// Run the main area loader (`bank7_code13`, `$C4CB`).
    LoadArea,
    /// Hand control to a Boot Stage table entry (`$C2D8 + stage`).
    BootTable(u8),
    /// Hand control to a Side Routine table entry (`$C22B + routine`).
    SideTable(u8),
    /// Run the sidescroll per-frame tail (`JMP L99E6`, `$D545`).
    SideFrame,
    /// Run the pause/pane path (`$C1CD`, sidescroll only).
    PausePane,
}

/// Current selectors read from RAM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selectors {
    /// `$0736`.
    pub mode: GameMode,
    /// `$076C`.
    pub stage: BootStage,
    /// `$0524`.
    pub routine: SideRoutine,
}

impl Selectors {
    /// Read `$0736`/`$076C`/`$0524` from live RAM.
    pub fn read(game: &Game) -> Self {
        Self {
            mode: GameMode::decode(game.ram[0x736]),
            stage: BootStage::decode(game.ram[0x76C]),
            routine: SideRoutine::decode(game.ram[0x524]),
        }
    }
}

/// Boot-wait exit predicate (`prg7.asm $C010-$C020`):
/// `(mode == $08 || mode == $14) && stage == Idle($01)`.
pub fn boot_ready(sel: Selectors) -> bool {
    matches!(sel.mode, GameMode::BootWait8 | GameMode::BootWait14)
        && matches!(sel.stage, BootStage::Idle)
}

/// Top-level dispatch for the `$C010` main loop: wait until [`boot_ready`],
/// then load the area. Mirrors the `LDA $0736 / CMP / LDA $076C` chain
/// without copying its branches.
pub fn dispatch_main_loop(sel: Selectors) -> Action {
    if boot_ready(sel) {
        Action::LoadArea
    } else {
        Action::KeepWaiting
    }
}

/// `LC2CA` (`$C2CA`) dispatch on Boot Stage: entry `stage` of
/// `bank7_pointer_table2` (`$C2D8`). Stages past the 7-entry table have no
/// ASM target (the original would read past the table — callers never do);
/// the dispatcher reports [`Action::KeepWaiting`] for those instead of
/// reproducing the overread.
pub fn dispatch_boot_stage(sel: Selectors) -> Action {
    match sel.stage {
        BootStage::BeyondTable(_) => Action::KeepWaiting,
        stage => Action::BootTable(stage.encode()),
    }
}

/// `LC220` (`$C220`) dispatch on Side Routine through
/// `bank7_pointer_table0` (`$C22B`). Like the ASM, flute state (`$0567`)
/// bypasses the table; unlike the ASM trampoline, the target is named.
pub fn dispatch_side_routine(sel: Selectors, flute_active: bool) -> Action {
    if flute_active {
        // $C223-$C226: BNE LC23A (skips the table when $0567 != 0).
        return Action::SideFrame;
    }
    match sel.routine {
        SideRoutine::Idle => Action::SideTable(0),
        SideRoutine::Dialog(_) | SideRoutine::Pane(_) | SideRoutine::Extended(_) => {
            Action::SideTable(sel.routine.encode())
        }
    }
}

/// NMI `$C157-$C177` routing on Side Routine + dialog type (`$074C`):
/// `None` dialog → pane/fast paths; dialog `>= 2` with routine outside
/// `3..7` → engine frame; else the NMI tail (timers/RNG/sprites).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NmiRoute {
    /// Take the pause/pane branch (`$C146-$C14B`).
    PauseBranch,
    /// Run the dialog-level fast path (`@End`, `$C155`/`$C162`).
    DialogFast,
    /// Run the engine tail (`$C169+`: timers, RNG, sprites).
    EngineTail,
}

/// Replicates the NMI `$C13A-$C177` decision tree on typed inputs.
///
/// `pause_requested` folds the `$C13A-$C149` chain: the pause pane runs
/// when `$0729 != 0` and movement is allowed (`$DE == 0`, or the Kasuto
/// door counters `$0763/$0764` are both 0). Working through the remaining
/// branches (`$C151 BEQ`, `$C155 BCC`, `$C15C BCC`, `$C160 BCS`,
/// `$C162 BCC`, `$C167 BNE`): every dialog/routine path reaches `@End`
/// except `routine == Idle($00)` with `dialog != $01` (level-up always
/// fast-paths, and any nonzero routine fast-paths).
pub fn route_nmi_tail(routine: SideRoutine, dialog_type: u8, pause_requested: bool) -> NmiRoute {
    if pause_requested {
        return NmiRoute::PauseBranch;
    }
    if matches!(routine, SideRoutine::Idle) && dialog_type != 0x01 {
        NmiRoute::EngineTail
    } else {
        NmiRoute::DialogFast
    }
}
