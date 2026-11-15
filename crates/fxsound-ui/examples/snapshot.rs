//! The windows drawn headless into PNG files, for looking at a layout without a display.
//!
//! Lays out the Pro window, the Lite window and every Settings pane with egui, as the application
//! would over a made-up state, tessellates what they paint and rasterises the meshes into a
//! `tiny_skia::Pixmap` — the triangles, their vertex colours and their textures (the font atlas
//! and the artwork) blended as egui's renderers blend them, premultiplied, in gamma space — then
//! writes each as a PNG. Nothing opens a window, and nothing touches a sound server.
//!
//! ```text
//! cargo run -p fxsound-ui --example snapshot
//! cargo run -p fxsound-ui --example snapshot -- --lang=ru,de --scale=2 --light
//! cargo run -p fxsound-ui --example snapshot -- --gilroy=C:\Fonts\Gilroy --out=shots
//! cargo run -p fxsound-ui --example snapshot -- --platform=windows --parity=interface
//! ```
//!
//! Flags: `--out=DIR` (default `target/snapshots`), `--lang=CODE,…` (default `en,de,ru,fi,pl,ua,fr`:
//! English and the languages whose labels run longest), `--scale=N` (pixels per point, default 1),
//! `--light`, `--platform=linux|windows` (whose Settings to draw: this build's by default),
//! `--parity=off|interface|sound` (the level of «Like the original FxSound LLC»), and
//! `--gilroy=PATH,…` — Gilroy files, or folders of them, that the user licensed: the windows are
//! then drawn in Gilroy as well, each file read as the application reads one. CrossFX has no
//! Gilroy of its own to draw them in. Without it, the General pane is also drawn with Gilroy
//! chosen and not found.
//!
//! The files are named `<window>-<lang>-<face>.png`: `pro`, `lite` and `settings-<tab>` for each
//! tab, and `settings-general-gilroy-missing` for the explanation the General pane gives then.

use egui::epaint::{ClippedPrimitive, Primitive};
use egui::{Color32, Pos2, RawInput, Rect, TextureId, Vec2, pos2};
use fxsound_core::{
    AppKey, AudioDevice, DeviceDirection, InterfaceFont, Platform, ThemeMode, ViewMode,
    WindowsParity,
};
use fxsound_ui::dialogs::settings::{
    AppLane, AppRow, DevicePriority, FontPane, GilroyFound, NavIcons, SettingsDialog,
    SettingsState, SettingsTab, WINDOW_SIZE,
};
use fxsound_ui::state::PresetEntry;
use fxsound_ui::typeface::{self, FaceReading, GilroyFaces};
use fxsound_ui::{AssetCache, Palette, UiState, ViewScratch, views};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| {
        args.iter()
            .find_map(|arg| arg.strip_prefix(name)?.strip_prefix('='))
    };
    let out = PathBuf::from(value("--out").unwrap_or("target/snapshots"));
    let languages: Vec<String> = value("--lang")
        .unwrap_or("en,de,ru,fi,pl,ua,fr")
        .split(',')
        .map(str::trim)
        .filter(|code| !code.is_empty())
        .map(str::to_owned)
        .collect();
    let scale: f32 = value("--scale")
        .and_then(|scale| scale.parse().ok())
        .filter(|scale: &f32| (0.5..=4.0).contains(scale))
        .unwrap_or(1.0);
    let theme = if args.iter().any(|arg| arg == "--light") {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    };
    let platform = match value("--platform") {
        Some("windows") => Platform::WINDOWS,
        Some("linux") => Platform::LINUX,
        _ => fxsound_core::PLATFORM,
    };
    let parity = value("--parity")
        .and_then(WindowsParity::parse)
        .unwrap_or_default()
        .offered_or_below();
    let gilroy = value("--gilroy").map(read_gilroy);

    if let Err(error) = std::fs::create_dir_all(&out) {
        eprintln!("{}: {error}", out.display());
        std::process::exit(1);
    }
    let mut faces: Vec<(&str, Option<&GilroyFaces>)> = vec![("manrope", None)];
    match &gilroy {
        Some(Ok(gilroy)) => faces.push(("gilroy", Some(gilroy))),
        Some(Err(why)) => eprintln!("not drawing in Gilroy: {why}"),
        None => {}
    }
    let mut written = 0;
    for language in &languages {
        if !fxsound_core::i18n::set_language(language) {
            eprintln!("no translation table for {language:?}; drawing it in English");
        }
        for (face, faces) in &faces {
            let shots = Shots {
                out: &out,
                language,
                face,
                scale,
                palette: Palette::new(theme).windows(parity.interface()),
                definitions: typeface::definitions(*faces),
            };
            written += shots.all(platform, parity, faces.is_some());
        }
    }
    println!("{written} images in {}", out.display());
}

/// The Gilroy at `paths`: files, or folders whose `.ttf` and `.otf` files are read.
fn read_gilroy(paths: &str) -> Result<GilroyFaces, String> {
    let mut files = Vec::new();
    for path in paths.split(',').map(PathBuf::from) {
        if path.is_dir() {
            let entries =
                std::fs::read_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let mut inside: Vec<PathBuf> =
                entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
            inside.sort();
            files.extend(inside);
        } else {
            files.push(path);
        }
    }
    let mut faces = Vec::new();
    for file in files {
        let is_font = file
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("ttf") || ext.eq_ignore_ascii_case("otf"));
        if !is_font {
            continue;
        }
        let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        match typeface::read_face(&bytes) {
            FaceReading::Gilroy(face) => faces.push((face, bytes)),
            other => eprintln!("{}: {other:?}, left out", file.display()),
        }
    }
    GilroyFaces::assemble(faces).map_err(|missing| format!("no Gilroy {missing:?}"))
}

/// One language in one face.
struct Shots<'a> {
    out: &'a Path,
    language: &'a str,
    face: &'a str,
    scale: f32,
    palette: Palette,
    definitions: egui::FontDefinitions,
}

impl Shots<'_> {
    /// Every window and pane; returns how many were written.
    fn all(&self, platform: Platform, parity: WindowsParity, in_gilroy: bool) -> usize {
        let mut written = 0;
        let mut state = demo_state();
        state.windows_parity = parity;
        for view in [ViewMode::Pro, ViewMode::Lite] {
            state.view = view;
            let name = match view {
                ViewMode::Pro => "pro",
                ViewMode::Lite => "lite",
            };
            let mut scratch = ViewScratch::new();
            let size = views::window_size(view);
            written += self.write(name, size, |ui, assets, _| {
                let _ = views::show(ui, &state, &mut scratch, self.palette, assets);
            });
        }
        for tab in SettingsTab::ALL {
            if !tab.offered_on(platform) {
                continue;
            }
            let mut settings = demo_settings(platform, tab, parity);
            if in_gilroy {
                settings.settings.interface_font = InterfaceFont::Gilroy;
                settings.font.gilroy = GilroyFound::Whole;
            }
            let name = format!("settings-{}", tab_name(tab));
            written += self.settings(&name, &settings);
            if tab == SettingsTab::General && !in_gilroy {
                settings.settings.interface_font = InterfaceFont::Gilroy;
                settings.font = FontPane {
                    gilroy: GilroyFound::None,
                    lacks_letters: false,
                    message: None,
                };
                written += self.settings("settings-general-gilroy-missing", &settings);
            }
        }
        written
    }

    fn settings(&self, name: &str, settings: &SettingsState) -> usize {
        let mut icons = NavIcons::new();
        self.write(name, WINDOW_SIZE, |ui, assets, _| {
            let outer = Rect::from_min_size(Pos2::ZERO, WINDOW_SIZE);
            let _ = SettingsDialog::new(settings).show(ui, outer, self.palette, assets, &mut icons);
        })
    }

    /// Lay `draw` out on a screen of `size` points, rasterise it and write it as `name`.
    fn write(
        &self,
        name: &str,
        size: Vec2,
        mut draw: impl FnMut(&mut egui::Ui, &mut AssetCache, ()),
    ) -> usize {
        let ctx = egui::Context::default();
        ctx.set_fonts(self.definitions.clone());
        ctx.set_pixels_per_point(self.scale);
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let mut assets = AssetCache::new();
        let mut textures = Textures::default();
        let input = || RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            ..Default::default()
        };
        // Three frames: the fonts and the artwork arrive in the first, and what is laid out from
        // what the first measured settles in the second.
        let mut primitives = Vec::new();
        for _ in 0..3 {
            let mut output = ctx.run_ui(input(), |ui| draw(ui, &mut assets, ()));
            textures.apply(&output.textures_delta);
            // Applied, here: egui insists on being told so.
            output.textures_delta.clear();
            primitives =
                ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
        }
        let width = (size.x * self.scale).round() as u32;
        let height = (size.y * self.scale).round() as u32;
        let mut canvas = Canvas::new(width, height);
        for primitive in &primitives {
            canvas.draw(primitive, &textures, self.scale);
        }
        let path = self
            .out
            .join(format!("{name}-{}-{}.png", self.language, self.face));
        match canvas.save(&path) {
            Ok(()) => 1,
            Err(error) => {
                eprintln!("{}: {error}", path.display());
                0
            }
        }
    }
}

fn tab_name(tab: SettingsTab) -> &'static str {
    match tab {
        SettingsTab::Audio => "audio",
        SettingsTab::General => "general",
        SettingsTab::Help => "help",
        SettingsTab::Microphone => "microphone",
        SettingsTab::Applications => "applications",
        SettingsTab::Experimental => "experimental",
    }
}

// ---------------------------------------------------------------------------------------------
// Rasterising egui's meshes
// ---------------------------------------------------------------------------------------------

/// The textures egui uploaded, as premultiplied RGBA in `0.0..=1.0`.
#[derive(Default)]
struct Textures {
    images: HashMap<TextureId, Texture>,
}

struct Texture {
    width: usize,
    height: usize,
    texels: Vec<[f32; 4]>,
}

impl Textures {
    fn apply(&mut self, delta: &egui::TexturesDelta) {
        for (id, change) in delta
            .set
            .iter()
            .flat_map(|(id, changes)| changes.iter().map(move |change| (id, change)))
        {
            let egui::ImageData::Color(image) = &change.image;
            let [width, height] = image.size;
            let texels: Vec<[f32; 4]> = image.pixels.iter().map(|&c| unit(c)).collect();
            match change.pos {
                None => {
                    self.images.insert(
                        *id,
                        Texture {
                            width,
                            height,
                            texels,
                        },
                    );
                }
                Some([x0, y0]) => {
                    if let Some(texture) = self.images.get_mut(id) {
                        for row in 0..height {
                            for column in 0..width {
                                let (x, y) = (x0 + column, y0 + row);
                                if x < texture.width && y < texture.height {
                                    texture.texels[y * texture.width + x] =
                                        texels[row * width + column];
                                }
                            }
                        }
                    }
                }
            }
        }
        for id in &delta.free {
            self.images.remove(id);
        }
    }

    fn get(&self, id: TextureId) -> Option<&Texture> {
        self.images.get(&id)
    }
}

impl Texture {
    /// The texel at `uv`, filtered bilinearly and clamped at the edges.
    fn sample(&self, uv: Pos2) -> [f32; 4] {
        let texture = self;
        let x = (uv.x * texture.width as f32 - 0.5).clamp(0.0, (texture.width - 1) as f32);
        let y = (uv.y * texture.height as f32 - 0.5).clamp(0.0, (texture.height - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = (
            (x0 + 1).min(texture.width - 1),
            (y0 + 1).min(texture.height - 1),
        );
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let at = |x: usize, y: usize| texture.texels[y * texture.width + x];
        let mut out = [0.0; 4];
        for (channel, value) in out.iter_mut().enumerate() {
            let top = at(x0, y0)[channel] * (1.0 - fx) + at(x1, y0)[channel] * fx;
            let bottom = at(x0, y1)[channel] * (1.0 - fx) + at(x1, y1)[channel] * fx;
            *value = top * (1.0 - fy) + bottom * fy;
        }
        out
    }
}

fn unit(colour: Color32) -> [f32; 4] {
    colour.to_array().map(|c| f32::from(c) / 255.0)
}

/// A premultiplied RGBA frame in `0.0..=1.0`, transparent where nothing was painted.
struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<[f32; 4]>,
}

impl Canvas {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![[0.0; 4]; width as usize * height as usize],
        }
    }

    fn draw(&mut self, clipped: &ClippedPrimitive, textures: &Textures, scale: f32) {
        let Primitive::Mesh(mesh) = &clipped.primitive else {
            return;
        };
        let clip = Rect::from_min_max(
            pos2(
                (clipped.clip_rect.min.x * scale).max(0.0),
                (clipped.clip_rect.min.y * scale).max(0.0),
            ),
            pos2(
                (clipped.clip_rect.max.x * scale).min(self.width as f32),
                (clipped.clip_rect.max.y * scale).min(self.height as f32),
            ),
        );
        if !clip.is_positive() {
            return;
        }
        let texture = textures.get(mesh.texture_id);
        for triangle in mesh.indices.as_chunks::<3>().0 {
            let [a, b, c] = triangle.map(|i| &mesh.vertices[i as usize]);
            self.triangle([a, b, c], texture, scale, clip);
        }
    }

    /// One triangle, filled by the top-left rule so that two triangles sharing an edge do not
    /// both blend the pixels on it.
    fn triangle(
        &mut self,
        vertices: [&egui::epaint::Vertex; 3],
        texture: Option<&Texture>,
        scale: f32,
        clip: Rect,
    ) {
        // An untextured shape samples one white texel at all three corners: sample it once.
        let flat = (vertices[0].uv == vertices[1].uv && vertices[1].uv == vertices[2].uv)
            .then(|| texture.map_or([1.0; 4], |texture| texture.sample(vertices[0].uv)));
        let sample = |uv: Pos2| match (flat, texture) {
            (Some(texel), _) => texel,
            (None, Some(texture)) => texture.sample(uv),
            (None, None) => [1.0; 4],
        };
        let p = vertices.map(|v| pos2(v.pos.x * scale, v.pos.y * scale));
        let edge =
            |a: Pos2, b: Pos2, c: Pos2| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
        let mut area = edge(p[0], p[1], p[2]);
        if area.abs() < 1e-9 {
            return;
        }
        // Wind every triangle the same way, which egui does not.
        let (order, sign) = if area > 0.0 {
            ([0, 1, 2], 1.0)
        } else {
            ([0, 2, 1], -1.0)
        };
        area *= sign;
        let p = order.map(|i| p[i]);
        let v = order.map(|i| vertices[i]);
        let x0 = p
            .iter()
            .map(|q| q.x)
            .fold(f32::MAX, f32::min)
            .max(clip.min.x)
            .floor() as i64;
        let x1 = p
            .iter()
            .map(|q| q.x)
            .fold(f32::MIN, f32::max)
            .min(clip.max.x)
            .ceil() as i64;
        let y0 = p
            .iter()
            .map(|q| q.y)
            .fold(f32::MAX, f32::min)
            .max(clip.min.y)
            .floor() as i64;
        let y1 = p
            .iter()
            .map(|q| q.y)
            .fold(f32::MIN, f32::max)
            .min(clip.max.y)
            .ceil() as i64;
        // The top-left rule, for a triangle wound with a positive area in screen coordinates
        // (y down): an edge owns its pixels when it is a top edge or a left one.
        let owns = |a: Pos2, b: Pos2| (a.y == b.y && b.x < a.x) || b.y < a.y;
        let owned = [owns(p[1], p[2]), owns(p[2], p[0]), owns(p[0], p[1])];
        let colours = v.map(|v| unit(v.color));
        for y in y0.max(0)..y1.min(i64::from(self.height)) {
            for x in x0.max(0)..x1.min(i64::from(self.width)) {
                let centre = pos2(x as f32 + 0.5, y as f32 + 0.5);
                if !clip.contains(centre) {
                    continue;
                }
                let w = [
                    edge(p[1], p[2], centre),
                    edge(p[2], p[0], centre),
                    edge(p[0], p[1], centre),
                ];
                let inside = w
                    .iter()
                    .zip(owned)
                    .all(|(&w, owned)| w > 0.0 || (w == 0.0 && owned));
                if !inside {
                    continue;
                }
                let w = w.map(|w| w / area);
                let uv = pos2(
                    w[0] * v[0].uv.x + w[1] * v[1].uv.x + w[2] * v[2].uv.x,
                    w[0] * v[0].uv.y + w[1] * v[1].uv.y + w[2] * v[2].uv.y,
                );
                let texel = sample(uv);
                let mut source = [0.0; 4];
                for (channel, value) in source.iter_mut().enumerate() {
                    let colour = w[0] * colours[0][channel]
                        + w[1] * colours[1][channel]
                        + w[2] * colours[2][channel];
                    *value = colour * texel[channel];
                }
                let target = &mut self.pixels[y as usize * self.width as usize + x as usize];
                for channel in 0..4 {
                    target[channel] = source[channel] + target[channel] * (1.0 - source[3]);
                }
            }
        }
    }

    fn save(&self, path: &Path) -> Result<(), String> {
        let mut pixmap = tiny_skia::Pixmap::new(self.width, self.height).ok_or("an empty image")?;
        for (target, source) in pixmap.pixels_mut().iter_mut().zip(&self.pixels) {
            let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            let alpha = byte(source[3]);
            let [r, g, b] = [0, 1, 2].map(|i| byte(source[i]).min(alpha));
            if let Some(pixel) = tiny_skia::PremultipliedColorU8::from_rgba(r, g, b, alpha) {
                *target = pixel;
            }
        }
        pixmap.save_png(path).map_err(|error| error.to_string())
    }
}

// ---------------------------------------------------------------------------------------------
// A made-up state, as `preview` has it
// ---------------------------------------------------------------------------------------------

fn device(id: u32, description: &str, direction: DeviceDirection) -> AudioDevice {
    AudioDevice {
        id,
        name: format!("snapshot.{id}"),
        description: description.to_owned(),
        is_default: id == 1,
        direction,
        form_factor: String::new(),
        is_virtual: false,
    }
}

fn presets(names: &[(&str, bool)]) -> Vec<PresetEntry> {
    names
        .iter()
        .map(|&(name, factory)| PresetEntry {
            name: name.to_owned(),
            factory,
            modified: false,
        })
        .collect()
}

fn demo_state() -> UiState {
    UiState {
        presets: presets(&[
            ("General", true),
            ("Music", true),
            ("Movies", true),
            ("Gaming", true),
            ("My Mix", false),
        ]),
        selected_preset: Some(1),
        devices: vec![
            device(
                1,
                "Ryzen HD Audio Controller Analogue Stereo",
                DeviceDirection::Output,
            ),
            device(
                2,
                "fifine Microphone Analogue Stereo",
                DeviceDirection::Output,
            ),
            device(
                3,
                "fifine Microphone Analogue Stereo",
                DeviceDirection::Input,
            ),
        ],
        selected_output: Some(0),
        selected_input: Some(2),
        effects: [3.0, 5.0, 7.0, 4.0, 8.0],
        master_gain_db: -4.0,
        volume_leveling: 1.5,
        filter_q: 2.0,
        balance_db: 6.0,
        audio_active: true,
        output_active: true,
        input_active: true,
        spectrum: [0.62, 0.71, 0.55, 0.48, 0.66, 0.52, 0.4, 0.33, 0.24, 0.15],
        ..UiState::default()
    }
}

fn priority_row(id: &str, name: &str, preset: Option<usize>, connected: bool) -> DevicePriority {
    DevicePriority {
        id: id.to_owned(),
        name: name.to_owned(),
        preset,
        connected,
        present: connected,
    }
}

fn demo_settings(platform: Platform, tab: SettingsTab, parity: WindowsParity) -> SettingsState {
    use DeviceDirection::{Input, Output};
    let app = |binary: &str, running: bool, lanes: &[(DeviceDirection, Option<&str>)]| AppRow {
        app: AppKey {
            binary: binary.to_owned(),
            name: binary.to_owned(),
            flatpak: String::new(),
        },
        name: binary.to_owned(),
        running,
        lanes: lanes
            .iter()
            .map(|&(direction, preset)| AppLane {
                direction,
                preset: preset.map(str::to_owned),
            })
            .collect(),
    };
    let mut state = SettingsState {
        tab,
        platform,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        presets: ["General", "Music", "Movies", "Gaming", "My Mix"]
            .map(str::to_owned)
            .to_vec(),
        input_presets: ["Clean Voice", "Headset", "Laptop Mic", "Podcast"]
            .map(str::to_owned)
            .to_vec(),
        apps: vec![
            app("Firefox", true, &[(Output, None)]),
            app("Discord", true, &[(Output, None), (Input, Some("Headset"))]),
            app("Spotify", false, &[(Output, Some("Music"))]),
        ],
        devices: vec![
            priority_row("alsa_output.usb-hecate", "HECATE G2000 Pro", Some(3), true),
            priority_row("alsa_output.pci-speakers", "Laptop Speakers", Some(1), true),
            priority_row("alsa_output.usb-hp", "HP Speakers", None, false),
        ],
        microphones: vec![
            priority_row("alsa_input.usb-fifine", "fifine Microphone", Some(3), true),
            priority_row(
                "alsa_input.pci-mic",
                "Built-in Audio Analogue Stereo",
                Some(2),
                true,
            ),
        ],
        has_microphone: true,
        ..SettingsState::default()
    };
    state.settings.windows_parity = parity;
    state
}
