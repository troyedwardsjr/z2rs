//! In-game options overlay (egui), opened with `O` or a gamepad's
//! LB + RB + Y (ZALiA opens its options menu the same way).
//!
//! Version choice: egui 0.36, the same egui the launcher uses, so the
//! enhancement widgets are one shared crate (`z2-enh-ui`). Input comes from
//! `egui-winit` 0.36, which is built on the winit 0.30 this frontend already
//! uses. `egui-wgpu` cannot be used: every egui-wgpu release that targets
//! winit 0.30 needs wgpu 22 or newer, while `pixels` 0.15 is pinned to wgpu
//! 0.19 (the last egui-wgpu on wgpu 0.19 is 0.27, which needs winit 0.29 and
//! an older egui than the launcher's). So [`Painter`] is a small wgpu 0.19
//! renderer for egui's tessellated meshes (one pipeline, one texture per egui
//! texture, scissored indexed draws), drawn on top of the game inside
//! `Pixels::render_with` after the viewport blit.
//!
//! Behaviour:
//!
//! * While open, the game is paused (and muted), except during netplay,
//!   where pausing one peer would stall the other: the game keeps running,
//!   the overlay shows the session status, and gameplay options are greyed
//!   out. Movie playback greys them out too (they would desync the movie).
//! * Display options (scale mode, fullscreen, display enhancements, volumes)
//!   apply at once. Gameplay options go through [`crate::app::apply_features`]
//!   (so `Game::set_enhancements` and the netplay identity stay in step);
//!   widescreen rebuilds the present path.
//! * Every change is written to the config file in use when the overlay
//!   closes (or the app quits).
//!
//! Nothing outside [`Painter`] and [`Overlay::attach`] needs a GPU or a
//! window, so the UI and the apply logic are unit tested headless.

use std::collections::HashMap;

use pixels::wgpu;
use z2_core::enh::{DisplayEnh, Enhancements};
use z2_enh_ui::{Group, Opts};

use crate::app::{apply_features, Display, Emu, Features, ScaleMode};
use crate::config::NativeConfig;

/// Overlay tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    /// Frontend options (window, scaling, widescreen, speed, audio).
    #[default]
    Game,
    /// Gameplay enhancements.
    Gameplay,
    /// Display-only enhancements.
    Graphics,
}

impl Tab {
    /// Every tab, in order.
    pub const ALL: [Tab; 3] = [Tab::Game, Tab::Gameplay, Tab::Graphics];

    /// Tab label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Tab::Game => "Game",
            Tab::Gameplay => "Gameplay",
            Tab::Graphics => "Graphics",
        }
    }

    /// The shared enhancement groups this tab shows.
    #[must_use]
    pub fn groups(self) -> &'static [Group] {
        match self {
            Tab::Game => &[Group::Audio],
            Tab::Gameplay => &[
                Group::Text,
                Group::Qol,
                Group::Fixes,
                Group::Enemies,
                Group::Abilities,
                Group::Rando,
                Group::Cheats,
            ],
            Tab::Graphics => &[Group::Graphics, Group::Dev],
        }
    }
}

/// Why gameplay options are read-only right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lock {
    /// Offline: everything is editable.
    #[default]
    None,
    /// An online session: both peers must run the same game.
    Netplay,
    /// A movie is playing: gameplay changes would desync it.
    Movie,
}

/// Widescreen presets the overlay offers (`(label, config value)`).
pub const WIDESCREEN_PRESETS: [(&str, &str); 4] = [
    ("Off (4:3)", "off"),
    ("16:10", "16:10"),
    ("16:9", "16:9"),
    ("21:9", "21:9"),
];

/// Largest fast-forward multiplier the overlay offers.
pub const MAX_FAST_FORWARD: u32 = 16;

/// Everything the overlay edits, captured from the running app each frame.
/// The UI edits a copy; [`apply`] turns the difference into changes.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    /// Gameplay enhancements in force.
    pub enhancements: Enhancements,
    /// Display-only enhancements in force.
    pub display_enh: DisplayEnh,
    /// The window is fullscreen.
    pub fullscreen: bool,
    /// How the picture fills the window.
    pub scale_mode: ScaleMode,
    /// Fast-forward multiplier (`Tab` held).
    pub fast_forward_multiplier: u32,
    /// Pause when the window loses focus.
    pub pause_on_focus_loss: bool,
    /// Widescreen margin tiles per side (0 = off).
    pub wide_tiles: u8,
    /// Why gameplay options (and widescreen, which can change gameplay) are
    /// read-only.
    pub lock: Lock,
    /// The game runs a ROM randomizer seed (`--seed` / `--rando-flags`), so
    /// the options that assume the original world are greyed out.
    pub rom_randomized: bool,
}

impl Model {
    /// Capture the current state.
    #[must_use]
    pub fn capture(
        config: &NativeConfig,
        feats: &Features,
        display: &Display,
        scale_mode: ScaleMode,
        fullscreen: bool,
        lock: Lock,
    ) -> Self {
        Self {
            enhancements: feats.enhancements,
            display_enh: display.settings().display_enh,
            fullscreen,
            scale_mode,
            fast_forward_multiplier: config.fast_forward_multiplier,
            pause_on_focus_loss: config.pause_on_focus_loss,
            wide_tiles: display.settings().wide_tiles,
            lock,
            rom_randomized: feats.rando.is_some(),
        }
    }

    fn gameplay_editable(&self) -> bool {
        self.lock == Lock::None
    }
}

/// What the app has to do itself after [`apply`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Applied {
    /// Enter (`true`) or leave fullscreen.
    pub fullscreen: Option<bool>,
    /// Something changed that belongs in the config file.
    pub config_changed: bool,
}

/// The app state [`apply`] writes to.
pub struct Targets<'a> {
    /// The config (written to disk later by the app).
    pub config: &'a mut NativeConfig,
    /// The running emulator.
    pub emu: &'a mut Emu,
    /// The features every rebuilt emulator gets.
    pub feats: &'a mut Features,
    /// The present path.
    pub display: &'a mut Display,
    /// The live scale mode.
    pub scale_mode: &'a mut ScaleMode,
}

/// Apply the edits between `before` and `after`. Gameplay edits (and
/// widescreen) are ignored while `before.lock` is set, whatever `after` says.
pub fn apply(before: &Model, after: &Model, t: Targets<'_>) -> Applied {
    let mut out = Applied::default();
    let offline = before.gameplay_editable();
    if offline && after.enhancements != before.enhancements {
        t.feats.enhancements = after.enhancements;
        apply_features(t.emu, *t.feats);
        t.config.enhancements = after.enhancements;
        out.config_changed = true;
    }
    if offline && after.wide_tiles != before.wide_tiles {
        let mut settings = t.display.settings().clone();
        settings.wide_tiles = after.wide_tiles;
        match Display::new(settings.clone()) {
            Ok(d) => {
                *t.display = d;
                t.feats.record = t.display.needs_record();
                t.feats.margin_sprites = settings.features(t.feats.coop).margin_sprites;
                t.feats.wide_gameplay = (t.config.widescreen_gameplay && after.wide_tiles > 0)
                    .then_some(after.wide_tiles);
                apply_features(t.emu, *t.feats);
                t.config.widescreen = WIDESCREEN_PRESETS
                    .iter()
                    .find(|(_, v)| z2_ppu::preset_tiles(v) == Some(after.wide_tiles))
                    .map_or_else(|| after.wide_tiles.to_string(), |(_, v)| (*v).to_string());
                out.config_changed = true;
            }
            Err(e) => eprintln!("options: widescreen change failed: {e}"),
        }
    }
    if after.display_enh != before.display_enh {
        let d = after.display_enh.clamped();
        t.display.set_display_enh(d);
        t.config.display_enh = d;
        out.config_changed = true;
    }
    if after.scale_mode != before.scale_mode {
        *t.scale_mode = after.scale_mode;
        t.config.scale_mode = after.scale_mode.as_str().to_string();
        out.config_changed = true;
    }
    if after.fast_forward_multiplier != before.fast_forward_multiplier {
        t.config.fast_forward_multiplier = after.fast_forward_multiplier.clamp(1, MAX_FAST_FORWARD);
        out.config_changed = true;
    }
    if after.pause_on_focus_loss != before.pause_on_focus_loss {
        t.config.pause_on_focus_loss = after.pause_on_focus_loss;
        out.config_changed = true;
    }
    if after.fullscreen != before.fullscreen {
        out.fullscreen = Some(after.fullscreen);
        t.config.fullscreen = after.fullscreen;
        out.config_changed = true;
    }
    out
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).weak());
}

/// The "Game" tab: the frontend's own options.
fn game_tab(ui: &mut egui::Ui, m: &mut Model, o: Opts) {
    ui.checkbox(&mut m.fullscreen, "Fullscreen")
        .on_hover_text("Borderless fullscreen (also F11, Alt+Enter or Cmd+Ctrl+F).");
    let mut integer = m.scale_mode == ScaleMode::Integer;
    if ui
        .checkbox(&mut integer, "Integer scaling")
        .on_hover_text(
            "Whole-number multiples only: the sharpest pixels, but may leave borders. Off \
             (fit) fills the window height.",
        )
        .changed()
    {
        m.scale_mode = if integer {
            ScaleMode::Integer
        } else {
            ScaleMode::Fit
        };
    }
    ui.horizontal(|ui| {
        let tip = "Extra picture at the sides. With wide gameplay on (config key \
                   widescreen_gameplay) enemies live out there too, so this is locked during \
                   netplay and movie playback.";
        ui.label("Widescreen").on_hover_text(tip);
        let current = WIDESCREEN_PRESETS
            .iter()
            .find(|(_, v)| z2_ppu::preset_tiles(v) == Some(m.wide_tiles))
            .map_or_else(
                || format!("{} tiles per side", m.wide_tiles),
                |(l, _)| (*l).to_string(),
            );
        ui.add_enabled_ui(o.gameplay_editable, |ui| {
            egui::ComboBox::from_id_salt("ovl-widescreen")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (label, v) in WIDESCREEN_PRESETS {
                        let tiles = z2_ppu::preset_tiles(v).unwrap_or(0);
                        ui.selectable_value(&mut m.wide_tiles, tiles, label);
                    }
                })
                .response
                .on_hover_text(tip);
        });
    });
    ui.horizontal(|ui| {
        let tip = "Game speed while Tab is held.";
        ui.label("Fast-forward").on_hover_text(tip);
        ui.add(egui::Slider::new(&mut m.fast_forward_multiplier, 1..=MAX_FAST_FORWARD).suffix("x"))
            .on_hover_text(tip);
    });
    ui.checkbox(
        &mut m.pause_on_focus_loss,
        "Pause when the window loses focus",
    )
    .on_hover_text("Ignored during netplay, where a paused peer would stall the other.");
}

/// Draw the options window's contents (tabs and the current tab). Separate
/// from [`Overlay`] so tests can draw every tab headless.
pub fn options_ui(ui: &mut egui::Ui, m: &mut Model, tab: &mut Tab, force_open: bool) {
    match m.lock {
        Lock::None => {}
        Lock::Netplay => hint(
            ui,
            "Online session: the game keeps running and gameplay options are read-only \
             (both players must use the same ones). Display options still apply.",
        ),
        Lock::Movie => hint(
            ui,
            "A movie is playing: gameplay options are read-only (they would desync it).",
        ),
    }
    ui.horizontal(|ui| {
        for t in Tab::ALL {
            ui.selectable_value(tab, t, t.label());
        }
    });
    ui.separator();
    let o = Opts {
        force_open,
        gameplay_editable: m.gameplay_editable(),
        rom_randomized: m.rom_randomized,
    };
    egui::ScrollArea::vertical()
        .auto_shrink([false, true])
        .show(ui, |ui| {
            match *tab {
                Tab::Game => game_tab(ui, m, o),
                Tab::Gameplay => {
                    z2_enh_ui::intro(ui, &m.enhancements);
                    z2_enh_ui::preset_buttons(
                        ui,
                        &mut m.enhancements,
                        &mut m.display_enh,
                        o.gameplay_editable,
                    );
                    hint(
                        ui,
                        "Changes apply at once. Start options (randomizer) take effect on \
                         the next new game.",
                    );
                }
                Tab::Graphics => hint(ui, "Display only: these never change the game."),
            }
            for g in tab.groups() {
                z2_enh_ui::group_ui(ui, *g, &mut m.enhancements, &mut m.display_enh, o);
            }
        });
}

/// The whole overlay frame: a dimmed backdrop and the options window.
/// `info` is a status line (the window title: speed, netplay state).
/// Returns `true` when the window's close button was pressed.
pub fn overlay_ui(
    ui: &mut egui::Ui,
    m: &mut Model,
    tab: &mut Tab,
    info: &str,
    force_open: bool,
) -> bool {
    let screen = ui.max_rect();
    ui.painter()
        .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(150));
    let mut open = true;
    let max = egui::vec2(
        (screen.width() - 24.0).clamp(200.0, 560.0),
        (screen.height() - 48.0).max(120.0),
    );
    egui::Window::new("Options")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .max_size(max)
        .default_width(max.x)
        .show(ui.ctx(), |ui| {
            ui.set_max_height(max.y - 40.0);
            hint(ui, info);
            hint(ui, "O or LB + RB + Y closes this menu.");
            options_ui(ui, m, tab, force_open);
        });
    !open
}

/// What [`Overlay::on_window_event`] did with an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventUse {
    /// Not the overlay's: handle it normally.
    Pass,
    /// The overlay used it: the game must not see it.
    Consumed,
    /// The overlay opened (`true`) or closed.
    Toggled(bool),
}

/// The overlay: egui context, winit input glue and the GPU painter.
pub struct Overlay {
    ctx: egui::Context,
    open: bool,
    tab: Tab,
    /// Input glue; `None` while there is no window.
    state: Option<egui_winit::State>,
    /// GPU side; `None` while there is no window.
    painter: Option<Painter>,
    /// Last frame's tessellated output, painted on the next present.
    prims: Vec<egui::ClippedPrimitive>,
    textures: egui::TexturesDelta,
    ppp: f32,
    /// The LB + RB + Y chord was held last poll (edge detection).
    chord_held: bool,
    /// Edits not yet written to the config file.
    dirty: bool,
}

impl std::fmt::Debug for Overlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Overlay")
            .field("open", &self.open)
            .field("tab", &self.tab)
            .field("attached", &self.painter.is_some())
            .finish()
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        // egui panics on dropping a texture delta nobody uploaded (e.g. a
        // frame run just before quitting).
        self.textures.clear();
    }
}

impl Default for Overlay {
    fn default() -> Self {
        Self::new()
    }
}

fn new_context() -> egui::Context {
    let ctx = egui::Context::default();
    ctx.set_visuals(egui::Visuals::dark());
    ctx
}

impl Overlay {
    /// A closed overlay with no window yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            ctx: new_context(),
            open: false,
            tab: Tab::default(),
            state: None,
            painter: None,
            prims: Vec::new(),
            textures: egui::TexturesDelta::default(),
            ppp: 1.0,
            chord_held: false,
            dirty: false,
        }
    }

    /// Whether the menu is showing.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether the game should stand still for the overlay (open, offline).
    #[must_use]
    pub fn pauses(&self, net_active: bool) -> bool {
        self.open && !net_active
    }

    /// Build the window-dependent parts (call after the `pixels` surface is
    /// made). A fresh egui context is used each time so the font atlas is
    /// uploaded to the new device.
    pub fn attach(&mut self, window: &winit::window::Window, pixels: &pixels::Pixels<'_>) {
        self.ctx = new_context();
        self.prims.clear();
        // Uploads meant for the old device are moot (a fresh context
        // re-sends everything); egui panics on dropping an unhandled delta.
        self.textures.clear();
        let device = pixels.device();
        self.state = Some(egui_winit::State::new(
            self.ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            Some(device.limits().max_texture_dimension_2d as usize),
        ));
        self.painter = Some(Painter::new(device, pixels.render_texture_format()));
    }

    /// Drop the window-dependent parts (Android `suspended`).
    pub fn detach(&mut self) {
        self.state = None;
        self.painter = None;
        self.prims.clear();
        self.textures.clear();
    }

    /// Open or close the menu.
    pub fn set_open(&mut self, open: bool) {
        self.open = open;
        if !open {
            self.prims.clear();
        } else if let Some(s) = self.state.as_mut() {
            // egui only gets winit events while open, so it never saw focus.
            s.egui_input_mut().focused = true;
        }
    }

    /// Feed the gamepad chord state (LB + RB + Y all held on some pad).
    /// Returns `true` on the press edge, i.e. when the menu should toggle.
    pub fn chord(&mut self, held: bool) -> bool {
        let edge = held && !self.chord_held;
        self.chord_held = held;
        edge
    }

    /// Take the "config needs saving" flag.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Route one window event. `O` toggles the menu; while it is open, `Esc`
    /// closes it and every keyboard, mouse and touch event is the menu's.
    pub fn on_window_event(
        &mut self,
        window: &winit::window::Window,
        event: &winit::event::WindowEvent,
    ) -> EventUse {
        use winit::event::{ElementState, WindowEvent};
        use winit::keyboard::{KeyCode, PhysicalKey};
        if let WindowEvent::KeyboardInput { event: k, .. } = event {
            if k.state == ElementState::Pressed && !k.repeat {
                let code = match k.physical_key {
                    PhysicalKey::Code(c) => Some(c),
                    PhysicalKey::Unidentified(_) => None,
                };
                let typing = self.open && self.ctx.egui_wants_keyboard_input();
                if code == Some(KeyCode::KeyO) && !typing
                    || code == Some(KeyCode::Escape) && self.open
                {
                    let open = !self.open;
                    self.set_open(open);
                    return EventUse::Toggled(open);
                }
            }
        }
        if !self.open {
            return EventUse::Pass;
        }
        let input = matches!(
            event,
            WindowEvent::KeyboardInput { .. }
                | WindowEvent::Ime(_)
                | WindowEvent::CursorMoved { .. }
                | WindowEvent::CursorEntered { .. }
                | WindowEvent::CursorLeft { .. }
                | WindowEvent::MouseWheel { .. }
                | WindowEvent::MouseInput { .. }
                | WindowEvent::Touch(_)
                | WindowEvent::PinchGesture { .. }
                | WindowEvent::ModifiersChanged(_)
        );
        if let Some(s) = self.state.as_mut() {
            let _ = s.on_window_event(window, event);
        }
        // Modifier changes still reach the game (fullscreen chords after
        // closing); everything else that is input stays in the menu.
        if input && !matches!(event, WindowEvent::ModifiersChanged(_)) {
            EventUse::Consumed
        } else {
            EventUse::Pass
        }
    }

    /// Run one UI frame over `model` (no-op while closed or detached).
    /// Returns `true` when the menu closed itself (its close button).
    pub fn run(&mut self, window: &winit::window::Window, model: &mut Model, info: &str) -> bool {
        if !self.open {
            return false;
        }
        let Some(state) = self.state.as_mut() else {
            return false;
        };
        let raw = state.take_egui_input(window);
        let tab = &mut self.tab;
        let mut closed = false;
        let out = self.ctx.run_ui(raw, |ui| {
            closed = overlay_ui(ui, model, tab, info, false);
        });
        state.handle_platform_output(window, out.platform_output);
        self.ppp = out.pixels_per_point;
        self.prims = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        self.textures.append(out.textures_delta);
        if closed {
            self.set_open(false);
        }
        closed
    }

    /// Record that `apply` changed something worth saving.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Paint the last UI frame over `target` (inside `Pixels::render_with`,
    /// after the game). Texture uploads happen even while closed, so the
    /// font atlas is never lost.
    pub fn paint(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        surface: (u32, u32),
    ) {
        let Some(p) = self.painter.as_mut() else {
            return;
        };
        let delta = std::mem::take(&mut self.textures);
        for (id, images) in &delta.set {
            for image in images {
                p.update_texture(device, queue, *id, image);
            }
        }
        if self.open && !self.prims.is_empty() {
            p.paint(
                device,
                queue,
                encoder,
                target,
                surface,
                self.ppp,
                &self.prims,
            );
        }
        for id in &delta.free {
            p.textures.remove(id);
        }
        // Handled above; egui panics on dropping an uncleared delta.
        let mut delta = delta;
        delta.clear();
    }
}

// ---------------------------------------------------------------------------
// GPU painter (wgpu 0.19)
// ---------------------------------------------------------------------------

/// egui's mesh shader, reduced from egui-wgpu's (MIT / Apache-2.0): vertex
/// colours and textures are sRGB-encoded premultiplied alpha.
const SHADER: &str = r"
struct VertexOutput {
    @location(0) tex_coord: vec2<f32>,
    @location(1) color: vec4<f32>,
    @builtin(position) position: vec4<f32>,
};

struct Locals {
    screen_size: vec2<f32>,
    _pad: vec2<f32>,
};
@group(0) @binding(0) var<uniform> r_locals: Locals;

fn linear_from_gamma_rgb(srgb: vec3<f32>) -> vec3<f32> {
    let cutoff = srgb < vec3<f32>(0.04045);
    let lower = srgb / vec3<f32>(12.92);
    let higher = pow((srgb + vec3<f32>(0.055)) / vec3<f32>(1.055), vec3<f32>(2.4));
    return select(higher, lower, cutoff);
}

fn unpack_color(color: u32) -> vec4<f32> {
    return vec4<f32>(
        f32(color & 255u),
        f32((color >> 8u) & 255u),
        f32((color >> 16u) & 255u),
        f32((color >> 24u) & 255u),
    ) / 255.0;
}

@vertex
fn vs_main(
    @location(0) a_pos: vec2<f32>,
    @location(1) a_tex_coord: vec2<f32>,
    @location(2) a_color: u32,
) -> VertexOutput {
    var out: VertexOutput;
    out.tex_coord = a_tex_coord;
    out.color = unpack_color(a_color);
    out.position = vec4<f32>(
        2.0 * a_pos.x / r_locals.screen_size.x - 1.0,
        1.0 - 2.0 * a_pos.y / r_locals.screen_size.y,
        0.0,
        1.0,
    );
    return out;
}

@group(1) @binding(0) var r_tex_color: texture_2d<f32>;
@group(1) @binding(1) var r_tex_sampler: sampler;

@fragment
fn fs_main_linear_framebuffer(in: VertexOutput) -> @location(0) vec4<f32> {
    let c = in.color * textureSample(r_tex_color, r_tex_sampler, in.tex_coord);
    return vec4<f32>(linear_from_gamma_rgb(c.rgb), c.a);
}

@fragment
fn fs_main_gamma_framebuffer(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color * textureSample(r_tex_color, r_tex_sampler, in.tex_coord);
}
";

/// Bytes per vertex: pos (2 x f32), uv (2 x f32), colour (4 x u8).
const VERTEX_BYTES: u64 = 20;

/// Serialize egui vertices in the layout the shader reads.
fn vertex_bytes(vertices: &[egui::epaint::Vertex], out: &mut Vec<u8>) {
    for v in vertices {
        for f in [v.pos.x, v.pos.y, v.uv.x, v.uv.y] {
            out.extend_from_slice(&f.to_le_bytes());
        }
        out.extend_from_slice(&v.color.to_array());
    }
}

/// Clip rectangle in points to a scissor rectangle in surface pixels, or
/// `None` when nothing of it is on the surface.
fn scissor(clip: egui::Rect, ppp: f32, surface: (u32, u32)) -> Option<(u32, u32, u32, u32)> {
    let x0 = (clip.min.x * ppp).round().clamp(0.0, surface.0 as f32) as u32;
    let y0 = (clip.min.y * ppp).round().clamp(0.0, surface.1 as f32) as u32;
    let x1 = (clip.max.x * ppp).round().clamp(0.0, surface.0 as f32) as u32;
    let y1 = (clip.max.y * ppp).round().clamp(0.0, surface.1 as f32) as u32;
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1 - x0, y1 - y0))
}

struct GpuTexture {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    size: [usize; 2],
}

/// A wgpu 0.19 renderer for egui's tessellated output.
pub struct Painter {
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    uniform_group: wgpu::BindGroup,
    texture_layout: wgpu::BindGroupLayout,
    textures: HashMap<egui::TextureId, GpuTexture>,
    vertex: Option<(wgpu::Buffer, u64)>,
    index: Option<(wgpu::Buffer, u64)>,
}

impl Painter {
    /// Build the pipeline for `format` (the `pixels` render target).
    #[must_use]
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("z2_overlay_shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("z2_overlay_locals"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("z2_overlay_uniform_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(16),
                },
                count: None,
            }],
        });
        let uniform_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("z2_overlay_uniform_group"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("z2_overlay_texture_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("z2_overlay_pipeline_layout"),
            bind_group_layouts: &[&uniform_layout, &texture_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("z2_overlay_pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: VERTEX_BYTES,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2,
                        1 => Float32x2,
                        2 => Uint32
                    ],
                }],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..wgpu::PrimitiveState::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: if format.is_srgb() {
                    "fs_main_linear_framebuffer"
                } else {
                    "fs_main_gamma_framebuffer"
                },
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::OneMinusDstAlpha,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
        });
        Self {
            pipeline,
            uniform,
            uniform_group,
            texture_layout,
            textures: HashMap::new(),
            vertex: None,
            index: None,
        }
    }

    fn update_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        id: egui::TextureId,
        delta: &egui::epaint::ImageDelta,
    ) {
        let egui::ImageData::Color(image) = &delta.image;
        let [w, h] = image.size;
        if w == 0 || h == 0 {
            return;
        }
        let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
        let origin = match delta.pos {
            Some([x, y]) => wgpu::Origin3d {
                x: x as u32,
                y: y as u32,
                z: 0,
            },
            None => {
                // A whole new texture (or a resized one).
                let filter = |f: egui::TextureFilter| match f {
                    egui::TextureFilter::Nearest => wgpu::FilterMode::Nearest,
                    egui::TextureFilter::Linear => wgpu::FilterMode::Linear,
                };
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("z2_overlay_texture"),
                    size: wgpu::Extent3d {
                        width: w as u32,
                        height: h as u32,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                    label: Some("z2_overlay_sampler"),
                    mag_filter: filter(delta.options.magnification),
                    min_filter: filter(delta.options.minification),
                    ..wgpu::SamplerDescriptor::default()
                });
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("z2_overlay_texture_group"),
                    layout: &self.texture_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&sampler),
                        },
                    ],
                });
                self.textures.insert(
                    id,
                    GpuTexture {
                        texture,
                        bind_group,
                        size: [w, h],
                    },
                );
                wgpu::Origin3d::ZERO
            }
        };
        let Some(t) = self.textures.get(&id) else {
            return;
        };
        if origin.x as usize + w > t.size[0] || origin.y as usize + h > t.size[1] {
            return;
        }
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &t.texture,
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * w as u32),
                rows_per_image: Some(h as u32),
            },
            wgpu::Extent3d {
                width: w as u32,
                height: h as u32,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Make sure `slot` holds a buffer of at least `size` bytes.
    fn ensure(
        device: &wgpu::Device,
        slot: &mut Option<(wgpu::Buffer, u64)>,
        size: u64,
        usage: wgpu::BufferUsages,
    ) {
        if slot.as_ref().is_some_and(|(_, cap)| *cap >= size) {
            return;
        }
        let cap = size.next_power_of_two().max(4096);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("z2_overlay_buffer"),
            size: cap,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        *slot = Some((buffer, cap));
    }

    #[allow(clippy::too_many_arguments)]
    fn paint(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        surface: (u32, u32),
        ppp: f32,
        prims: &[egui::ClippedPrimitive],
    ) {
        // Everything in one vertex and one index buffer; each mesh is drawn
        // with its own base vertex and scissor.
        let mut vbytes = Vec::new();
        let mut ibytes = Vec::new();
        let mut draws = Vec::new();
        let mut base_vertex = 0i32;
        let mut first_index = 0u32;
        for p in prims {
            let egui::epaint::Primitive::Mesh(mesh) = &p.primitive else {
                continue;
            };
            if mesh.indices.is_empty() {
                continue;
            }
            vertex_bytes(&mesh.vertices, &mut vbytes);
            for i in &mesh.indices {
                ibytes.extend_from_slice(&i.to_le_bytes());
            }
            let n = mesh.indices.len() as u32;
            if let Some(rect) = scissor(p.clip_rect, ppp, surface) {
                draws.push((
                    mesh.texture_id,
                    rect,
                    first_index..first_index + n,
                    base_vertex,
                ));
            }
            base_vertex += mesh.vertices.len() as i32;
            first_index += n;
        }
        if draws.is_empty() {
            return;
        }
        Self::ensure(
            device,
            &mut self.vertex,
            vbytes.len() as u64,
            wgpu::BufferUsages::VERTEX,
        );
        Self::ensure(
            device,
            &mut self.index,
            ibytes.len() as u64,
            wgpu::BufferUsages::INDEX,
        );
        let (Some((vbuf, _)), Some((ibuf, _))) = (self.vertex.as_ref(), self.index.as_ref()) else {
            return;
        };
        queue.write_buffer(vbuf, 0, &vbytes);
        queue.write_buffer(ibuf, 0, &ibytes);
        let screen = [
            surface.0 as f32 / ppp,
            surface.1 as f32 / ppp,
            0.0f32,
            0.0f32,
        ];
        let mut locals = [0u8; 16];
        for (chunk, v) in locals.as_chunks_mut::<4>().0.iter_mut().zip(screen) {
            chunk.copy_from_slice(&v.to_le_bytes());
        }
        queue.write_buffer(&self.uniform, 0, &locals);

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("z2_overlay_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.uniform_group, &[]);
        pass.set_vertex_buffer(0, vbuf.slice(..vbytes.len() as u64));
        pass.set_index_buffer(ibuf.slice(..ibytes.len() as u64), wgpu::IndexFormat::Uint32);
        for (tex, (x, y, w, h), range, base) in draws {
            let Some(t) = self.textures.get(&tex) else {
                continue;
            };
            pass.set_scissor_rect(x, y, w, h);
            pass.set_bind_group(1, &t.bind_group, &[]);
            pass.draw_indexed(range, base, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{DisplaySettings, Features};

    fn model(lock: Lock) -> Model {
        Model {
            enhancements: Enhancements::default(),
            display_enh: DisplayEnh::default(),
            fullscreen: false,
            scale_mode: ScaleMode::Fit,
            fast_forward_multiplier: 4,
            pause_on_focus_loss: true,
            wide_tiles: 0,
            lock,
            rom_randomized: false,
        }
    }

    /// Every tab draws headless, editable and locked, with every group
    /// open, and an idle frame edits nothing.
    #[test]
    fn overlay_ui_renders_headless_for_every_tab_and_group() {
        let ctx = new_context();
        for lock in [Lock::None, Lock::Netplay, Lock::Movie] {
            for t in Tab::ALL {
                let mut m = model(lock);
                let before = m.clone();
                let mut tab = t;
                for _ in 0..2 {
                    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                        let closed = overlay_ui(ui, &mut m, &mut tab, "z2rs", true);
                        assert!(!closed);
                    });
                    out.textures_delta.clear();
                    assert!(!out.shapes.is_empty());
                    let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
                    assert!(!prims.is_empty());
                }
                assert_eq!(tab, t);
                assert_eq!(m, before, "an idle frame leaves the options alone");
            }
        }
    }

    #[test]
    fn every_group_is_on_exactly_one_tab() {
        for g in Group::ALL {
            let n = Tab::ALL.iter().filter(|t| t.groups().contains(&g)).count();
            assert_eq!(n, 1, "{g:?}");
        }
    }

    #[test]
    fn chord_toggles_on_the_press_edge_only() {
        let mut o = Overlay::new();
        assert!(!o.chord(false));
        assert!(o.chord(true));
        assert!(!o.chord(true), "held is not a second press");
        assert!(!o.chord(false));
        assert!(o.chord(true));
    }

    #[test]
    fn open_overlay_pauses_offline_only() {
        let mut o = Overlay::new();
        assert!(!o.pauses(false));
        o.set_open(true);
        assert!(o.pauses(false));
        assert!(!o.pauses(true), "netplay keeps running");
    }

    #[test]
    fn scissor_clamps_to_the_surface() {
        let r = egui::Rect::from_min_max(egui::pos2(-5.0, 10.0), egui::pos2(50.0, 500.0));
        assert_eq!(scissor(r, 2.0, (80, 400)), Some((0, 20, 80, 380)));
        let off = egui::Rect::from_min_max(egui::pos2(100.0, 0.0), egui::pos2(120.0, 10.0));
        assert_eq!(scissor(off, 1.0, (80, 400)), None);
    }

    #[test]
    fn vertex_layout_is_twenty_bytes() {
        let v = egui::epaint::Vertex {
            pos: egui::pos2(1.0, 2.0),
            uv: egui::pos2(0.5, 0.25),
            color: egui::Color32::from_rgba_premultiplied(1, 2, 3, 4),
        };
        let mut b = Vec::new();
        vertex_bytes(&[v, v], &mut b);
        assert_eq!(b.len() as u64, 2 * VERTEX_BYTES);
        assert_eq!(&b[16..20], &[1, 2, 3, 4]);
        assert_eq!(f32::from_le_bytes(b[4..8].try_into().unwrap()), 2.0);
    }

    /// The shader must validate with the naga that pixels' wgpu 0.19 uses.
    #[test]
    fn shader_validates() {
        let module = naga::front::wgsl::parse_str(SHADER).expect("WGSL parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("WGSL validates");
        let names: Vec<&str> = module
            .entry_points
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        for n in [
            "vs_main",
            "fs_main_linear_framebuffer",
            "fs_main_gamma_framebuffer",
        ] {
            assert!(names.contains(&n), "{names:?}");
        }
    }

    /// Edits reach the running game, the display and the config, and the
    /// config written to disk reads back with them.
    #[test]
    fn apply_updates_game_display_and_persisted_config() {
        let mut config = NativeConfig::default();
        let mut emu = crate::app::new_emu(44_100);
        let mut feats = Features::default();
        let mut display = Display::new(DisplaySettings {
            scale: 1,
            ..DisplaySettings::default()
        })
        .unwrap();
        let mut scale_mode = ScaleMode::Fit;
        let before = Model::capture(&config, &feats, &display, scale_mode, false, Lock::None);
        let mut after = before.clone();
        after.enhancements = Enhancements::zalia_preset();
        after.display_enh = DisplayEnh::zalia_preset();
        after.scale_mode = ScaleMode::Integer;
        after.fast_forward_multiplier = 8;
        after.pause_on_focus_loss = false;
        after.fullscreen = true;
        after.wide_tiles = 11;
        let applied = apply(
            &before,
            &after,
            Targets {
                config: &mut config,
                emu: &mut emu,
                feats: &mut feats,
                display: &mut display,
                scale_mode: &mut scale_mode,
            },
        );
        assert!(applied.config_changed);
        assert_eq!(applied.fullscreen, Some(true));
        assert_eq!(*emu.game.enhancements(), Enhancements::zalia_preset());
        assert_eq!(feats.enhancements, Enhancements::zalia_preset());
        assert_eq!(display.settings().display_enh, DisplayEnh::zalia_preset());
        assert_eq!(display.settings().wide_tiles, 11);
        assert!(feats.record, "widescreen arms the render record");
        assert_eq!(feats.wide_gameplay, Some(11));
        assert_eq!(scale_mode, ScaleMode::Integer);

        let dir = std::env::temp_dir().join(format!("z2-overlay-cfg-{}", std::process::id()));
        let path = dir.join("z2-native.json");
        config.save_to(&path).unwrap();
        let back = NativeConfig::load_from(&path);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(back.enhancements, Enhancements::zalia_preset());
        assert_eq!(back.display_enh, DisplayEnh::zalia_preset());
        assert_eq!(back.scale_mode, "integer");
        assert_eq!(back.fast_forward_multiplier, 8);
        assert!(!back.pause_on_focus_loss);
        assert!(back.fullscreen);
        assert_eq!(back.widescreen, "16:9");
        assert_eq!(back.widescreen_tiles(), 11);
    }

    /// While locked (netplay, movie), gameplay edits and widescreen are
    /// dropped; display edits still apply.
    #[test]
    fn locked_overlay_never_changes_gameplay() {
        for lock in [Lock::Netplay, Lock::Movie] {
            let mut config = NativeConfig::default();
            let mut emu = crate::app::new_emu(44_100);
            let mut feats = Features::default();
            let mut display = Display::new(DisplaySettings {
                scale: 1,
                ..DisplaySettings::default()
            })
            .unwrap();
            let mut scale_mode = ScaleMode::Fit;
            let before = Model::capture(&config, &feats, &display, scale_mode, false, lock);
            let mut after = before.clone();
            after.enhancements.cheats.invincible = true;
            after.wide_tiles = 11;
            after.display_enh.screen_shake = true;
            let trapset = emu.trapset_id;
            apply(
                &before,
                &after,
                Targets {
                    config: &mut config,
                    emu: &mut emu,
                    feats: &mut feats,
                    display: &mut display,
                    scale_mode: &mut scale_mode,
                },
            );
            assert_eq!(*emu.game.enhancements(), Enhancements::default());
            assert_eq!(emu.trapset_id, trapset, "identity untouched");
            assert_eq!(config.enhancements, Enhancements::default());
            assert_eq!(display.settings().wide_tiles, 0);
            assert!(display.settings().display_enh.screen_shake);
            assert!(config.display_enh.screen_shake);
        }
    }
}
