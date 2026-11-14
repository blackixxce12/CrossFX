//! The tray on Windows: an icon in the notification area (`tray-icon`), with the menu, the states
//! and the tooltip of the StatusNotifierItem the Linux build registers (`crate::tray`).
//!
//! The model is the same: [`TrayState`] is the mirror the controller pushes ([`TrayHandle::update`],
//! through `events::fan_out`), and a click only ever *sends* a [`TrayCommand`] to the GUI thread,
//! which wakes for it. What differs is how it is drawn:
//!
//! - **Its own thread.** The icon's hidden window, and the menu's, belong to the thread that made
//!   them, which has to run a message loop for them; the GUI thread's own loop does not run while
//!   the window is hidden, which is when the tray matters most. So `fxsound-tray` makes the icon
//!   and pumps its messages, and an update reaches it as a posted message to a window of its own,
//!   which is dispatched even inside the menu's modal loop and applied once that loop is over —
//!   a menu is never replaced under the user's pointer.
//! - **Ticks, not radio groups.** A Win32 menu is the original's (`FxSystemTrayView.cpp:216-330`):
//!   a tick on the preset, the device and the theme in use, a separator between the factory and
//!   the user presets. Every row is rebuilt from the mirror after a click, since the menu ticks a
//!   row itself when it is clicked, whether or not the click changed anything.
//! - **A left click toggles the window**, as `NIN_SELECT` does in the original
//!   (`FxSystemTrayView.cpp:446-455`), and a double click counts once — the second click of it is
//!   not a second toggle that would close the window it just opened. The right button opens the
//!   menu.
//! - **The icon** is the same SVG rasterised at the notification area's size, and the white bars
//!   of "on" are drawn dark on a light taskbar, where white would not be seen. The original keyed
//!   its icons to its own theme; the taskbar's is the one the icon is drawn on.
//! - **The tooltip** is the original's, cut at the 127 characters the notification area keeps.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread::{self, JoinHandle};

use fxsound_core::i18n::tr;
use fxsound_core::{DeviceDirection, ThemeMode, WindowsLook};
use tray_icon::menu::{
    CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};
use tray_icon::{
    MouseButton, MouseButtonState, TrayIcon as NotifyIcon, TrayIconBuilder, TrayIconEvent,
};
use windows::Win32::Foundation::{ERROR_SUCCESS, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, GetSystemMetricsForDpi,
    SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowW, GetMessageW,
    MSG, PostMessageW, PostQuitMessage, RegisterClassW, SM_CXSMICON, TranslateMessage,
    WINDOW_EX_STYLE, WM_APP, WM_CLOSE, WM_DESTROY, WM_SETTINGCHANGE, WNDCLASSW, WS_EX_TOOLWINDOW,
    WS_OVERLAPPED,
};
use windows::core::{PCWSTR, w};

use crate::tray::{
    TrayCommand, TrayIcon, TrayState, device_menu_label, device_row_label, preset_label,
    preset_menu_label, rasterise_rgba,
};
use crate::wake::WakingSender;

use super::wide;

/// Posted to the tray thread's window: a new mirror is waiting in [`Shared::pending`].
const WM_REDRAW: u32 = WM_APP + 1;
/// Posted to the tray thread's window: take the icon away and end the thread.
const WM_QUIT_TRAY: u32 = WM_APP + 2;

/// How many UTF-16 units of the tooltip the notification area keeps (`szTip` is 128 with its
/// terminating NUL).
const TOOLTIP_UNITS: usize = 127;

/// What the bars of "on" are drawn with on a light taskbar: the near-black of the system's own
/// glyphs there.
const ON_LIGHT_TASKBAR: &str = "#1f1f1f";

/// Why there is no tray.
#[derive(Debug)]
pub struct TrayError(String);

impl std::fmt::Display for TrayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TrayError {}

/// What the GUI thread and the tray thread share.
#[derive(Debug)]
struct Shared {
    /// The newest mirror: what [`TrayHandle::update`] changes, and what the tray thread draws.
    latest: Mutex<TrayState>,
    /// `latest` changed since the tray thread last drew it.
    changed: AtomicBool,
    /// The tray thread has ended, or is ending.
    closed: AtomicBool,
}

impl Shared {
    /// The newest mirror, if the tray thread has not drawn it yet.
    fn take_changed(&self) -> Option<TrayState> {
        self.changed.swap(false, Ordering::SeqCst).then(|| {
            self.latest
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        })
    }
}

/// The GUI thread's end of the tray.
#[derive(Debug)]
pub struct TrayHandle {
    shared: Arc<Shared>,
    /// The tray thread's window, as an address (see [`crate::win::session`]).
    window: isize,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl TrayHandle {
    /// Whether the icon is on screen: the taskbar is there to show it. Windows' notification area
    /// always has room — an icon it does not show beside the clock is one click away, behind the
    /// arrow — so the question here is only whether Explorer's taskbar is running at all.
    #[must_use]
    pub fn is_visible(&self) -> bool {
        !self.is_closed() && taskbar_present()
    }

    /// Change the mirror and have the tray draw it. Returns `false` when the tray is gone —
    /// mutating a `TrayState` anywhere else does nothing at all, because this call is what makes
    /// a change visible.
    pub fn update(&self, f: impl FnOnce(&mut TrayState)) -> bool {
        if self.is_closed() {
            return false;
        }
        f(&mut self
            .shared
            .latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner));
        self.shared.changed.store(true, Ordering::SeqCst);
        self.post(WM_REDRAW)
    }

    /// The icon and the tooltip for `power` and `processing`, as `setStatus` sets both.
    pub fn set_status(&self, power: bool, processing: bool) -> bool {
        self.update(|state| {
            state.power = power;
            state.processing = processing;
        })
    }

    /// `true` once the tray thread has ended.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::SeqCst)
    }

    /// Take the icon away (`Shell_NotifyIcon(NIM_DELETE)`, `FxSystemTrayView.cpp:56-59`) and end
    /// the tray thread. Returns once it has, so it is safe to call on the way out of `main`.
    pub fn shutdown(&self) {
        let thread = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(thread) = thread {
            if self.post(WM_QUIT_TRAY) {
                if thread.join().is_err() {
                    log::warn!("the tray's thread panicked");
                }
            } else {
                log::warn!("the tray's thread could not be told to stop");
            }
        }
        self.shared.closed.store(true, Ordering::SeqCst);
    }

    fn post(&self, message: u32) -> bool {
        // SAFETY: posting to the tray thread's window, which may be gone, which the post says.
        unsafe {
            PostMessageW(
                Some(HWND(self.window as *mut std::ffi::c_void)),
                message,
                WPARAM(0),
                LPARAM(0),
            )
        }
        .is_ok()
    }
}

impl Drop for TrayHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The tray is redrawn from the controller's events, the whole mirror at once
/// (`crate::events::fan_out`).
impl crate::events::TraySink for TrayHandle {
    fn redraw(&self, state: TrayState) {
        self.update(|mirror| *mirror = state);
    }
}

/// Put the icon in the notification area and start serving it from a thread of its own.
///
/// # Errors
///
/// If the thread, its window or the icon cannot be made.
pub fn spawn(
    state: TrayState,
    tx: impl Into<WakingSender<TrayCommand>>,
) -> Result<TrayHandle, TrayError> {
    let tx = tx.into();
    let shared = Arc::new(Shared {
        latest: Mutex::new(state.clone()),
        changed: AtomicBool::new(false),
        closed: AtomicBool::new(false),
    });
    let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
    let thread = {
        let shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("fxsound-tray".to_owned())
            .spawn(move || run(state, &tx, &shared, &ready_tx))
            .map_err(|err| TrayError(format!("the tray's thread did not start: {err}")))?
    };
    match ready_rx.recv() {
        Ok(Ok(window)) => Ok(TrayHandle {
            shared,
            window,
            thread: Mutex::new(Some(thread)),
        }),
        Ok(Err(why)) => {
            let _ = thread.join();
            Err(TrayError(why))
        }
        Err(_) => {
            let _ = thread.join();
            Err(TrayError(
                "the tray's thread ended before the icon was made".to_owned(),
            ))
        }
    }
}

/// Whether Explorer's taskbar, and so its notification area, is running.
fn taskbar_present() -> bool {
    // SAFETY: a lookup by class name; the window handle is not kept.
    unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) }
        .is_ok_and(|window| !window.is_invalid())
}

/// Whether the taskbar is drawn light: `SystemUsesLightTheme` under the user's theme settings,
/// which Windows 11 sets to 1 for its light theme and Windows 10 leaves at 0 by default.
fn taskbar_is_light() -> bool {
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let name = wide("SystemUsesLightTheme");
    let mut value = 0_u32;
    let mut size = u32::try_from(size_of::<u32>()).unwrap_or(4);
    // SAFETY: NUL-terminated strings and a `u32` the size it is said to be.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&raw mut size),
        )
    };
    status == ERROR_SUCCESS && value != 0
}

/// The notification area's icon size: the small icon of the system's DPI — 16 pixels at 100 %,
/// 24 at 150 %, 32 at 200 %.
fn icon_size() -> u32 {
    // SAFETY: plain queries. The thread is per-monitor aware (see `run`), so the DPI is the real one.
    let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) };
    u32::try_from(size).unwrap_or(16).clamp(16, 256)
}

/// The four icons, rasterised once at the notification area's size, as straight RGBA.
struct Icons {
    size: u32,
    off: Vec<u8>,
    on_dark: Vec<u8>,
    on_light: Vec<u8>,
    processing: Vec<u8>,
}

impl Icons {
    fn rasterised(size: u32) -> Self {
        let draw = |svg: &[u8]| rasterise_rgba(svg, size).unwrap_or_default();
        let on = TrayIcon::On.svg();
        let on_light = String::from_utf8_lossy(on)
            .replace("#ffffff", ON_LIGHT_TASKBAR)
            .into_bytes();
        Self {
            size,
            off: draw(TrayIcon::Off.svg()),
            on_dark: draw(on),
            on_light: draw(&on_light),
            processing: draw(TrayIcon::Processing.svg()),
        }
    }

    fn icon(&self, icon: TrayIcon, light: bool) -> Option<tray_icon::Icon> {
        let rgba = match (icon, light) {
            (TrayIcon::Off, _) => &self.off,
            (TrayIcon::On, false) => &self.on_dark,
            (TrayIcon::On, true) => &self.on_light,
            (TrayIcon::Processing, _) => &self.processing,
        };
        if rgba.is_empty() {
            return None;
        }
        tray_icon::Icon::from_rgba(rgba.clone(), self.size, self.size)
            .map_err(|err| log::warn!("the tray icon could not be made: {err}"))
            .ok()
    }
}

/// The tooltip, as `setStatus` composes it (`TrayState::tooltip`), cut to what the notification
/// area keeps.
#[must_use]
pub fn tooltip(state: &TrayState) -> String {
    let full = state.tooltip();
    if full.encode_utf16().count() <= TOOLTIP_UNITS {
        return full;
    }
    let mut cut = String::new();
    let mut units = 0;
    for c in full.chars() {
        // One unit is kept for the ellipsis.
        if units + c.len_utf16() > TOOLTIP_UNITS - 1 {
            break;
        }
        units += c.len_utf16();
        cut.push(c);
    }
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

/// A label as a Win32 menu shows it: `&` marks the next letter as the item's access key there, so
/// a preset called "Rock & Roll" needs its `&` doubled to be shown at all.
fn menu_text(label: &str) -> String {
    label.replace('&', "&&")
}

/// One row of the menu, before it becomes a Win32 menu: what [`build_menu`] makes, and what the
/// tests read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A plain item.
    Item(String, TrayCommand),
    /// An item with a tick when it is the one in use.
    Check(String, bool, TrayCommand),
    Separator,
    Submenu(String, Vec<Row>),
}

/// The tree of `showContextMenu` (`FxSystemTrayView.cpp:216-330`) for two lanes, in the order the
/// Linux tray lists it: Open, the power, a preset submenu per lane, the device submenus between
/// separators, Settings, Theme, Exit. The preset submenus go with the power at «Как в Windows» =
/// Interface and above, as on Windows ([`WindowsLook::PresetsNeedPower`]).
#[must_use]
pub fn menu_rows(state: &TrayState) -> Vec<Row> {
    let mut rows = vec![
        Row::Item(tr("Open"), TrayCommand::Open),
        Row::Item(
            if state.power {
                tr("Turn Off")
            } else {
                tr("Turn On")
            },
            TrayCommand::SetPower(!state.power),
        ),
    ];
    let presets_shown = state.power
        || !state
            .windows_parity
            .windows_look(WindowsLook::PresetsNeedPower);
    if presets_shown {
        rows.extend(
            DeviceDirection::ALL
                .into_iter()
                .filter_map(|direction| preset_rows(state, direction)),
        );
    }
    let devices: Vec<Row> = DeviceDirection::ALL
        .into_iter()
        .filter_map(|direction| device_rows(state, direction))
        .collect();
    if !devices.is_empty() {
        rows.push(Row::Separator);
        rows.extend(devices);
        rows.push(Row::Separator);
    }
    rows.push(Row::Item(tr("Settings"), TrayCommand::OpenSettings));
    // Each label a literal of its own, which the translation audit reads.
    rows.push(Row::Submenu(
        tr("Theme"),
        [
            (ThemeMode::Dark, tr("Dark")),
            (ThemeMode::Light, tr("Light")),
        ]
        .into_iter()
        .map(|(mode, label)| Row::Check(label, state.theme == mode, TrayCommand::SetTheme(mode)))
        .collect(),
    ));
    rows.push(Row::Item(tr("Exit"), TrayCommand::Exit));
    rows
}

/// A lane's presets: the factory ones, a separator, the user's, the one in use ticked.
fn preset_rows(state: &TrayState, direction: DeviceDirection) -> Option<Row> {
    let lane = state.lane(direction);
    if lane.presets.is_empty() {
        return None;
    }
    let row = |(index, preset): (usize, &crate::tray::TrayPreset)| {
        Row::Check(
            preset_label(preset),
            lane.selected_preset == Some(index),
            TrayCommand::SelectPreset {
                direction,
                name: preset.name.clone(),
            },
        )
    };
    let factory: Vec<Row> = lane
        .presets
        .iter()
        .enumerate()
        .filter(|(_, preset)| preset.factory)
        .map(row)
        .collect();
    let user: Vec<Row> = lane
        .presets
        .iter()
        .enumerate()
        .filter(|(_, preset)| !preset.factory)
        .map(row)
        .collect();
    let mut rows = factory;
    if !rows.is_empty() && !user.is_empty() {
        rows.push(Row::Separator);
    }
    rows.extend(user);
    Some(Row::Submenu(preset_menu_label(direction), rows))
}

/// A lane's devices: `Off`, then the direction's devices, the lane's ticked — and nothing ticked
/// for a lane on a device the list does not carry right now, which is not off.
fn device_rows(state: &TrayState, direction: DeviceDirection) -> Option<Row> {
    let lane = state.lane(direction);
    let devices: Vec<(usize, &crate::tray::TrayDevice)> = state
        .devices
        .iter()
        .enumerate()
        .filter(|(_, device)| device.direction == direction)
        .collect();
    if devices.is_empty() {
        return None;
    }
    let off = lane.device.is_none() && lane.unlisted.is_none();
    let rows = std::iter::once(Row::Check(tr("Off"), off, TrayCommand::Detach(direction)))
        .chain(devices.into_iter().map(|(index, device)| {
            Row::Check(
                device_row_label(state, direction, &device.name),
                lane.device == Some(index),
                TrayCommand::SelectDevice {
                    direction,
                    node_name: device.node_name.clone(),
                },
            )
        }))
        .collect();
    Some(Row::Submenu(device_menu_label(direction), rows))
}

/// The Win32 menu for `rows`, and what each of its items sends.
fn build_menu(rows: &[Row]) -> (Menu, HashMap<MenuId, TrayCommand>) {
    let menu = Menu::new();
    let mut commands = HashMap::new();
    for row in rows {
        if let Some(item) = build_item(row, &mut commands)
            && let Err(err) = menu.append(item.as_ref())
        {
            log::warn!("a tray menu row could not be added: {err}");
        }
    }
    (menu, commands)
}

fn build_item(
    row: &Row,
    commands: &mut HashMap<MenuId, TrayCommand>,
) -> Option<Box<dyn IsMenuItem>> {
    let mut id = |command: &TrayCommand| {
        let id = MenuId::new(format!("fxsound-{}", commands.len()));
        commands.insert(id.clone(), command.clone());
        id
    };
    Some(match row {
        Row::Item(label, command) => {
            Box::new(MenuItem::with_id(id(command), menu_text(label), true, None))
        }
        Row::Check(label, checked, command) => Box::new(CheckMenuItem::with_id(
            id(command),
            menu_text(label),
            true,
            *checked,
            None,
        )),
        Row::Separator => Box::new(PredefinedMenuItem::separator()),
        Row::Submenu(label, rows) => {
            let submenu = Submenu::new(menu_text(label), true);
            for row in rows {
                if let Some(item) = build_item(row, commands)
                    && let Err(err) = submenu.append(item.as_ref())
                {
                    log::warn!("a tray menu row could not be added: {err}");
                }
            }
            Box::new(submenu)
        }
    })
}

thread_local! {
    /// Set by the tray thread's window when the system's settings changed — the taskbar's theme
    /// among them — and read by its loop.
    static SETTINGS_CHANGED: Cell<bool> = const { Cell::new(false) };
}

/// The tray thread's own window: a target for [`WM_REDRAW`] and [`WM_QUIT_TRAY`], and top-level
/// rather than message-only so that it hears the theme change (`WM_SETTINGCHANGE` is broadcast).
unsafe extern "system" fn tray_window(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: called by Windows on the window's own thread, for the window.
    unsafe {
        match message {
            WM_REDRAW => LRESULT(0),
            WM_SETTINGCHANGE => {
                SETTINGS_CHANGED.set(true);
                DefWindowProcW(window, message, wparam, lparam)
            }
            WM_QUIT_TRAY => {
                let _ = DestroyWindow(window);
                LRESULT(0)
            }
            // `taskkill` without `/F`, or an installer closing FxSound, sends every top-level
            // window of the process `WM_CLOSE`. The session watcher hears it as the tray's Exit
            // (`crate::win::session`); this window stays, and the icon with it, until that way
            // out takes them away with `WM_QUIT_TRAY`.
            WM_CLOSE => LRESULT(0),
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(window, message, wparam, lparam),
        }
    }
}

fn create_window() -> Result<HWND, String> {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    let class_name = w!("FxSoundTray");
    // SAFETY: this module's own handle.
    let instance = unsafe { GetModuleHandleW(None) }.ok().map(Into::into);
    REGISTERED.get_or_init(|| {
        let class = WNDCLASSW {
            lpfnWndProc: Some(tray_window),
            hInstance: instance.unwrap_or_default(),
            lpszClassName: class_name,
            ..WNDCLASSW::default()
        };
        // SAFETY: `class` outlives the call.
        unsafe { RegisterClassW(&raw const class) };
    });
    // SAFETY: the class is registered; the window is this thread's, never shown. Its title is
    // the port's, not FxSound LLC's FxSound's (see `crate::win::session`).
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0),
            class_name,
            w!("FxSound Port tray"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            instance,
            None,
        )
    }
    .map_err(|err| format!("the tray's window could not be made: {err}"))
}

/// What the tray thread keeps between two messages.
struct Tray {
    /// The mirror drawn now.
    state: TrayState,
    icons: Icons,
    light: bool,
    commands: HashMap<MenuId, TrayCommand>,
    /// The next left button release belongs to a double click: not a second toggle.
    swallow_release: bool,
}

impl Tray {
    fn send(tx: &WakingSender<TrayCommand>, command: TrayCommand) {
        if tx.try_send(command).is_err() {
            log::warn!("dropped a tray command: the application is not reading them");
        }
    }

    /// Everything the last dispatched message may have left: clicks on the icon and the menu,
    /// a mirror to draw, a theme to follow.
    fn after_dispatch(
        &mut self,
        icon: &NotifyIcon,
        tx: &WakingSender<TrayCommand>,
        shared: &Shared,
    ) {
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            match event {
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } => self.swallow_release = true,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } => {
                    let second_of_a_double_click = std::mem::take(&mut self.swallow_release);
                    if !second_of_a_double_click {
                        Self::send(tx, TrayCommand::ToggleWindow);
                    }
                }
                _ => {}
            }
        }
        let mut clicked = false;
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            clicked = true;
            if let Some(command) = self.commands.get(event.id()) {
                Self::send(tx, command.clone());
            }
        }
        if let Some(state) = shared.take_changed() {
            self.draw(icon, state, clicked);
        } else if clicked {
            // The menu ticked the row it was clicked on; the mirror says what is ticked.
            self.draw(icon, self.state.clone(), true);
        }
        if SETTINGS_CHANGED.take() {
            let light = taskbar_is_light();
            if light != self.light {
                self.light = light;
                self.set_icon(icon);
            }
        }
    }

    fn set_icon(&self, icon: &NotifyIcon) {
        if let Err(err) = icon.set_icon(self.icons.icon(self.state.icon(), self.light)) {
            log::warn!("the tray icon could not be changed: {err}");
        }
    }

    /// Draw `state`: the icon, the tooltip and the menu, each only when it changed — or the menu
    /// whatever changed when `rebuild`.
    fn draw(&mut self, icon: &NotifyIcon, state: TrayState, rebuild: bool) {
        let old = std::mem::replace(&mut self.state, state);
        if old.icon() != self.state.icon() {
            self.set_icon(icon);
        }
        let tip = tooltip(&self.state);
        if tooltip(&old) != tip
            && let Err(err) = icon.set_tooltip(Some(tip))
        {
            log::warn!("the tray tooltip could not be changed: {err}");
        }
        // Whether audio is flowing changes with every song, and nothing in the menu shows it.
        let menu_of = |state: &TrayState| TrayState {
            processing: false,
            ..state.clone()
        };
        if rebuild || menu_of(&old) != menu_of(&self.state) {
            let (menu, commands) = build_menu(&menu_rows(&self.state));
            icon.set_menu(Some(Box::new(menu)));
            self.commands = commands;
        }
    }
}

/// The tray thread: make the window and the icon, say how that went on `ready`, then pump the
/// messages of both until told to stop.
fn run(
    state: TrayState,
    tx: &WakingSender<TrayCommand>,
    shared: &Shared,
    ready: &crossbeam_channel::Sender<Result<isize, String>>,
) {
    // SAFETY: for this thread only; the icon is sized for the DPI it then reads.
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let window = match create_window() {
        Ok(window) => window,
        Err(why) => {
            let _ = ready.send(Err(why));
            shared.closed.store(true, Ordering::SeqCst);
            return;
        }
    };
    let light = taskbar_is_light();
    let icons = Icons::rasterised(icon_size());
    let (menu, commands) = build_menu(&menu_rows(&state));
    let mut builder = TrayIconBuilder::new()
        .with_tooltip(tooltip(&state))
        .with_menu(Box::new(menu))
        // The left button toggles the window; the right one opens the menu.
        .with_menu_on_left_click(false);
    if let Some(image) = icons.icon(state.icon(), light) {
        builder = builder.with_icon(image);
    }
    let icon = match builder.build() {
        Ok(icon) => icon,
        Err(err) => {
            // SAFETY: the window made above, on this thread.
            let _ = unsafe { DestroyWindow(window) };
            let _ = ready.send(Err(format!("the tray icon could not be made: {err}")));
            shared.closed.store(true, Ordering::SeqCst);
            return;
        }
    };
    // Where the shell put it, as `Shell_NotifyIconGetRect` says: the log's word that it is there,
    // as the Linux tray logs a watcher coming. None while Explorer's taskbar is not running; the
    // icon is added when it starts (`TaskbarCreated`).
    match icon.rect() {
        Some(rect) => log::info!(
            "the tray icon is in the notification area, {}×{} at ({}, {})",
            rect.size.width,
            rect.size.height,
            rect.position.x,
            rect.position.y
        ),
        None => log::info!("the tray icon waits for the notification area"),
    }
    let mut tray = Tray {
        state,
        icons,
        light,
        commands,
        swallow_release: false,
    };
    let _ = ready.send(Ok(window.0 as isize));
    let mut msg = MSG::default();
    // SAFETY: the loop of the thread the windows belong to.
    unsafe {
        while GetMessageW(&raw mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
            tray.after_dispatch(&icon, tx, shared);
        }
    }
    shared.closed.store(true, Ordering::SeqCst);
    // `NIM_DELETE`, and the icon's own window with it.
    drop(icon);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tray::{TrayDevice, TrayLane, TrayPreset};
    use fxsound_core::WindowsParity;

    const OUT: DeviceDirection = DeviceDirection::Output;
    const IN: DeviceDirection = DeviceDirection::Input;

    fn preset(name: &str, factory: bool, modified: bool) -> TrayPreset {
        TrayPreset {
            name: name.to_owned(),
            factory,
            modified,
        }
    }

    fn device(name: &str, direction: DeviceDirection) -> TrayDevice {
        TrayDevice {
            name: name.to_owned(),
            node_name: format!("node.{name}"),
            direction,
        }
    }

    fn populated() -> TrayState {
        TrayState {
            output: TrayLane {
                presets: vec![
                    preset("General", true, false),
                    preset("Bass Booster", true, false),
                    preset("My Mix", false, true),
                ],
                selected_preset: Some(2),
                device: Some(0),
                unlisted: None,
            },
            input: TrayLane {
                presets: vec![preset("Podcast", true, false)],
                selected_preset: Some(0),
                device: None,
                unlisted: None,
            },
            devices: vec![
                device("Speakers (Realtek(R) Audio)", OUT),
                device("Headphones", OUT),
                device("Microphone Array", IN),
            ],
            ..TrayState::default()
        }
    }

    fn labels(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Item(label, _) | Row::Check(label, _, _) | Row::Submenu(label, _) => {
                    label.clone()
                }
                Row::Separator => "---".to_owned(),
            })
            .collect()
    }

    fn submenu<'a>(rows: &'a [Row], name: &str) -> &'a [Row] {
        rows.iter()
            .find_map(|row| match row {
                Row::Submenu(label, rows) if label == name => Some(rows.as_slice()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {name} in {:?}", labels(rows)))
    }

    fn ticked(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .filter_map(|row| match row {
                Row::Check(label, true, _) => Some(label.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_menu_has_the_linux_trays_rows_in_its_order() {
        assert_eq!(
            labels(&menu_rows(&populated())),
            [
                "Open",
                "Turn Off",
                "Output Presets",
                "Input Presets",
                "---",
                "Playback Device Select",
                "Recording Device Select",
                "---",
                "Settings",
                "Theme",
                "Exit",
            ]
        );
    }

    #[test]
    fn the_presets_are_split_where_their_kind_changes_and_only_the_one_in_use_is_ticked() {
        let rows = menu_rows(&populated());
        let presets = submenu(&rows, "Output Presets");
        assert_eq!(
            labels(presets),
            ["General", "Bass Booster", "---", "My Mix *"]
        );
        assert_eq!(ticked(presets), ["My Mix *"]);
        let Some(Row::Check(_, _, command)) = presets.first() else {
            panic!("a preset row");
        };
        assert_eq!(
            *command,
            TrayCommand::SelectPreset {
                direction: OUT,
                name: "General".to_owned()
            },
            "a pick carries the name, not the row"
        );
    }

    #[test]
    fn each_lane_ticks_its_device_or_off_and_an_unlisted_device_ticks_nothing() {
        let mut state = populated();
        let rows = menu_rows(&state);
        assert_eq!(
            ticked(submenu(&rows, "Playback Device Select")),
            ["Speakers (Realtek(R) Audio)"]
        );
        assert_eq!(ticked(submenu(&rows, "Recording Device Select")), ["Off"]);

        state.output.device = None;
        state.output.unlisted = Some("Bluetooth Headset".to_owned());
        let rows = menu_rows(&state);
        assert!(ticked(submenu(&rows, "Playback Device Select")).is_empty());
    }

    #[test]
    fn at_interface_the_presets_go_with_the_power_as_on_windows() {
        let state = TrayState {
            power: false,
            windows_parity: WindowsParity::Interface,
            ..populated()
        };
        let labels = labels(&menu_rows(&state));
        assert_eq!(labels[1], "Turn On");
        assert!(!labels.contains(&"Output Presets".to_owned()), "{labels:?}");
        let off = TrayState {
            power: false,
            ..populated()
        };
        assert!(
            labels_of(&off).contains(&"Output Presets".to_owned()),
            "below Interface they stay (0.4.0 audit R7)"
        );
    }

    fn labels_of(state: &TrayState) -> Vec<String> {
        labels(&menu_rows(state))
    }

    #[test]
    fn an_ampersand_in_a_name_is_shown_and_not_taken_for_an_access_key() {
        assert_eq!(menu_text("Rock & Roll"), "Rock && Roll");
        assert_eq!(menu_text("Plain"), "Plain");
    }

    #[test]
    fn the_tooltip_is_the_originals_cut_to_what_the_notification_area_keeps() {
        let state = populated();
        assert_eq!(tooltip(&state), state.tooltip(), "short enough as it is");
        let mut long = populated();
        long.devices[0].name = "A very long device name that goes on and on ".repeat(5);
        let tip = tooltip(&long);
        assert!(tip.encode_utf16().count() <= TOOLTIP_UNITS, "{tip}");
        assert!(tip.ends_with('…'), "{tip}");
        assert!(tip.starts_with(&long.status_line()));
    }

    #[test]
    fn the_icons_are_drawn_at_the_size_asked_and_on_is_dark_on_a_light_taskbar() {
        let icons = Icons::rasterised(16);
        for rgba in [
            &icons.off,
            &icons.on_dark,
            &icons.on_light,
            &icons.processing,
        ] {
            assert_eq!(rgba.len(), 16 * 16 * 4);
            assert!(
                rgba.chunks(4).any(|pixel| pixel[3] > 0),
                "something is drawn"
            );
        }
        let brightest = |rgba: &[u8]| {
            rgba.chunks(4)
                .filter(|pixel| pixel[3] == 255)
                .map(|pixel| u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2]))
                .max()
                .unwrap_or(0)
        };
        assert!(
            brightest(&icons.on_dark) > 700,
            "white bars on a dark taskbar"
        );
        assert!(brightest(&icons.on_light) < 150, "dark bars on a light one");
    }
}
