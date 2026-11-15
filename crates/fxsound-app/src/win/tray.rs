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
//! - **Everything wakes the loop.** The loop acts once `GetMessageW` has returned a *posted*
//!   message, and much of what reaches the tray is *sent*: the broadcast `WM_SETTINGCHANGE` of a
//!   theme change (which cannot be posted at all), Explorer's `TaskbarCreated`, and the shell's
//!   callbacks for the icon. A sent message is handled inside `GetMessageW`, which then goes on
//!   waiting, so what it left behind waited for some unrelated message: a switch to the light
//!   theme left white bars on the light taskbar until the sound next started or stopped, and a
//!   click that came with no pointer moves to start tray-icon's timer — a tap on a touch screen,
//!   the keyboard's Enter on the icon — could wait as long. Whatever a window procedure,
//!   tray-icon or muda reports is recorded (`Pending`) and followed by a posted `WM_WAKE`
//!   (`record`), as tray-icon asks of a loop of one's own.
//! - **No hover tracking.** tray-icon follows the pointer over the icon with a 15 ms timer, for
//!   enter and leave events FxSound has no use for, and its timer outlived the pointer — after
//!   every right click, among others — and woke the tray thread some sixty times a second for as
//!   long as FxSound ran on. The pointer's moves over the icon are kept from it (`icon_window`),
//!   so the timer never starts; the clicks still reach it.
//! - **A taskbar that comes back** — Explorer restarted — has none of the old one's icons.
//!   tray-icon adds FxSound's again from what the shell last took, which is not what FxSound
//!   shows now when a change came while there was no taskbar to take it. So the tray keeps what
//!   the shell took apart from the mirror (`Shown`), sets again whatever differs at every look,
//!   and looks once tray-icon has added the icon to a new taskbar.
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

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock, PoisonError};
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
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowW, GetMessageW,
    MSG, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SM_CXSMICON,
    TranslateMessage, WINDOW_EX_STYLE, WM_APP, WM_CLOSE, WM_DESTROY, WM_MOUSEMOVE, WM_NCDESTROY,
    WM_SETTINGCHANGE, WNDCLASSW, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
};
use windows::core::{PCWSTR, w};

use crate::tray::{
    TrayCommand, TrayIcon, TrayState, device_menu_label, device_row_label, preset_label,
    preset_menu_label, rasterise_rgba,
};
use crate::wake::WakingSender;

use super::wide;

/// Posted to the tray thread's window: a new mirror is waiting in [`Shared::latest`].
const WM_REDRAW: u32 = WM_APP + 1;
/// Posted to the tray thread's window: take the icon away and end the thread.
const WM_QUIT_TRAY: u32 = WM_APP + 2;
/// Posted to the tray thread's window by [`record`]: something is waiting in [`Pending`].
const WM_WAKE: u32 = WM_APP + 3;

/// tray-icon's callback message for its icon (`WM_USER_TRAYICON` in tray-icon 0.26), which
/// carries the notification area's mouse message in `lParam`. Should a later tray-icon number it
/// otherwise, the moves [`icon_window`] keeps from it reach it again, and its hover timer with
/// them.
const TRAY_ICON_CALLBACK: u32 = 6002;

/// FxSound's subclass of tray-icon's window, beside muda's: `SetWindowSubclass` tells the two
/// apart by procedure and id.
const ICON_SUBCLASS: usize = 1;

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

/// One row of the menu, before it becomes a Win32 menu: what `build_menu` makes, and what the
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

/// A left-button click on the icon, as the loop acts on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Click {
    /// The button went up: the window is toggled, unless this ends a double click.
    Release,
    /// The second press of a double click.
    Double,
}

impl Click {
    /// The click `event` is, if the loop acts on it: the left button's release and double click.
    fn of(event: &TrayIconEvent) -> Option<Self> {
        match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } => Some(Self::Release),
            TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } => Some(Self::Double),
            _ => None,
        }
    }
}

/// What happened on the tray thread for its loop to act on: recorded where it happened — in a
/// window procedure, or in tray-icon's and muda's event handlers, which run inside one — and
/// taken by the loop after the message that woke it ([`Tray::after_dispatch`]).
#[derive(Debug, Default)]
struct Pending {
    /// Left-button clicks on the icon, in the order they came.
    clicks: Vec<Click>,
    /// The menu items picked, in the order they were.
    picks: Vec<MenuId>,
    /// The system's settings changed, the taskbar's theme among them.
    settings_changed: bool,
    /// Explorer made a new taskbar, and tray-icon has added the icon to it again.
    taskbar_created: bool,
}

thread_local! {
    /// What the tray thread's window procedures and event handlers recorded since its loop last
    /// looked.
    static PENDING: RefCell<Pending> = RefCell::new(Pending::default());
    /// The tray thread's own window, through which [`record`] wakes the loop; null on any other
    /// thread.
    static WAKE: Cell<isize> = const { Cell::new(0) };
}

/// Record something for the tray thread's loop, and wake it (see the module docs): a posted
/// [`WM_WAKE`] makes `GetMessageW` return, which a message Windows *sends* does not.
///
/// Only notes are taken here, never the tray's own state: this runs inside window procedures, and
/// some of those are called while the loop is in the middle of a change of its own, since a call
/// to the shell lets sent messages in.
fn record(note: impl FnOnce(&mut Pending)) {
    PENDING.with_borrow_mut(note);
    let window = WAKE.get();
    if window != 0 {
        // SAFETY: posting to this thread's own window, which a failure would say is gone.
        let posted = unsafe {
            PostMessageW(
                Some(HWND(window as *mut c_void)),
                WM_WAKE,
                WPARAM(0),
                LPARAM(0),
            )
        };
        if let Err(err) = posted {
            log::warn!("the tray's thread could not be woken: {err}");
        }
    }
}

/// Have tray-icon's icon events and muda's menu events recorded for the loop ([`record`]) rather
/// than queued in their channels, which wake nothing. Once per process, before the first icon is
/// made: each handler is the process's, and the first event sent without one leaves it at none.
fn route_events() {
    static ROUTED: Once = Once::new();
    ROUTED.call_once(|| {
        TrayIconEvent::set_event_handler(Some(on_icon_event));
        MenuEvent::set_event_handler(Some(on_menu_event));
    });
}

/// tray-icon's report of the pointer on the icon: the clicks the loop acts on are recorded, and
/// nothing else is — the pointer entering and leaving, which FxSound does not follow.
fn on_icon_event(event: TrayIconEvent) {
    if let Some(click) = Click::of(&event) {
        record(|pending| pending.clicks.push(click));
    }
}

/// muda's report of a menu item picked.
fn on_menu_event(event: MenuEvent) {
    record(|pending| pending.picks.push(event.id));
}

/// The message Explorer broadcasts to every top-level window once it has made a new taskbar,
/// registered once per process; `None` if it cannot be.
fn taskbar_created() -> Option<u32> {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    // SAFETY: a NUL-terminated name.
    let message = *MESSAGE.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) });
    (message != 0).then_some(message)
}

/// tray-icon's window, the one the icon belongs to, as FxSound subclasses it
/// ([`guard_icon_window`]):
///
/// - the pointer's moves over the icon stop here, before tray-icon starts its hover timer for
///   them (see the module docs); the clicks go on;
/// - so does `WM_CLOSE`, which tray-icon leaves to `DefWindowProcW`: that would destroy the window
///   and the icon with it, for a closer that sends every top-level window of the process
///   `WM_CLOSE`. The session watcher hears such a close as the tray's Exit
///   (`crate::win::session`), whose way out takes the icon away (`WM_QUIT_TRAY`);
/// - `TaskbarCreated` goes to tray-icon first, which adds the icon to the new taskbar, and is then
///   recorded for the loop, which sets what the icon is to show over what tray-icon added back.
unsafe extern "system" fn icon_window(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass: usize,
    _data: usize,
) -> LRESULT {
    // SAFETY: called by Windows on the window's own thread, for the window, while the subclass is
    // installed; it takes itself away at the window's last message.
    unsafe {
        match message {
            TRAY_ICON_CALLBACK if lparam.0 as u32 == WM_MOUSEMOVE => LRESULT(0),
            WM_CLOSE => LRESULT(0),
            WM_NCDESTROY => {
                let _ = RemoveWindowSubclass(window, Some(icon_window), ICON_SUBCLASS);
                DefSubclassProc(window, message, wparam, lparam)
            }
            _ if Some(message) == taskbar_created() => {
                let answer = DefSubclassProc(window, message, wparam, lparam);
                record(|pending| pending.taskbar_created = true);
                answer
            }
            _ => DefSubclassProc(window, message, wparam, lparam),
        }
    }
}

/// Subclass tray-icon's `window` ([`icon_window`]), on the thread it belongs to. A failure is a
/// log line: the icon then works as tray-icon alone has it.
fn guard_icon_window(window: HWND) {
    // SAFETY: a window of this thread's, and a procedure with the signature subclasses have.
    if !unsafe { SetWindowSubclass(window, Some(icon_window), ICON_SUBCLASS, 0) }.as_bool() {
        log::warn!(
            "the tray icon's window could not be subclassed: its hover timer may run on, and a \
             taskbar that restarts may show an icon from before"
        );
    }
}

/// The tray thread's own window: a target for [`WM_REDRAW`], [`WM_WAKE`] and [`WM_QUIT_TRAY`],
/// and top-level rather than message-only so that it hears the theme change (`WM_SETTINGCHANGE`
/// is broadcast).
unsafe extern "system" fn tray_window(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: called by Windows on the window's own thread, for the window.
    unsafe {
        match message {
            WM_REDRAW | WM_WAKE => LRESULT(0),
            // Sent, never posted: the loop is woken for it.
            WM_SETTINGCHANGE => {
                record(|pending| pending.settings_changed = true);
                DefWindowProcW(window, message, wparam, lparam)
            }
            WM_QUIT_TRAY => {
                let _ = DestroyWindow(window);
                LRESULT(0)
            }
            // `taskkill` without `/F` reaches only the session watcher's window, and an installer
            // closes FxSound through the Restart Manager, which ends its session
            // (`crate::win::session`): neither sends this window `WM_CLOSE`. A closer that sends
            // every top-level window of the process one reaches the session watcher's too, which
            // hears it as the tray's Exit; this window stays, and so does the icon's
            // ([`icon_window`]), until that way out takes them away with `WM_QUIT_TRAY`.
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

/// The notification area's icon as the tray thread changes it: tray-icon's ([`Notification`]),
/// or a test's stand-in.
trait Notify {
    /// Show the picture of `icon`, drawn for a light taskbar or a dark one. Whether the shell
    /// took it.
    fn picture(&self, icon: TrayIcon, light: bool) -> bool;
    /// Show `tip` as the tooltip. Whether the shell took it.
    fn tip(&self, tip: &str) -> bool;
    /// Put `menu` behind the right button.
    fn menu(&self, menu: Menu);
}

/// tray-icon's icon, with the pictures rasterised for it.
struct Notification {
    icon: NotifyIcon,
    icons: Icons,
}

impl Notify for Notification {
    fn picture(&self, icon: TrayIcon, light: bool) -> bool {
        self.icon
            .set_icon(self.icons.icon(icon, light))
            .map_err(|err| log::warn!("the tray icon could not be changed: {err}"))
            .is_ok()
    }

    fn tip(&self, tip: &str) -> bool {
        self.icon
            .set_tooltip(Some(tip))
            .map_err(|err| log::warn!("the tray tooltip could not be changed: {err}"))
            .is_ok()
    }

    fn menu(&self, menu: Menu) {
        self.icon.set_menu(Some(Box::new(menu)));
    }
}

/// What the notification area shows, as far as FxSound knows: the picture and the tooltip the
/// shell last took. `None` for one it refused — there was no taskbar, say — or that a new taskbar
/// may show otherwise, which the next look sets again ([`Tray::show`]).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Shown {
    /// The icon and whether it was drawn for a light taskbar.
    picture: Option<(TrayIcon, bool)>,
    tip: Option<String>,
}

/// What the tray thread keeps between two messages.
struct Tray {
    /// The mirror: what the notification area is to show.
    state: TrayState,
    /// Whether the taskbar is drawn light.
    light: bool,
    /// How that is read: [`taskbar_is_light`], or a test's answer.
    taskbar_light: fn() -> bool,
    /// What the notification area shows, which is not the mirror while the shell refuses a change.
    shown: Shown,
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

    /// Everything the last dispatched message may have left ([`Pending`]): clicks on the icon
    /// and the menu, a mirror to draw, a theme to follow, a taskbar that came back.
    fn after_dispatch(
        &mut self,
        icon: &impl Notify,
        tx: &WakingSender<TrayCommand>,
        shared: &Shared,
    ) {
        let pending = PENDING.with_borrow_mut(std::mem::take);
        for click in pending.clicks {
            match click {
                Click::Double => self.swallow_release = true,
                Click::Release => {
                    let second_of_a_double_click = std::mem::take(&mut self.swallow_release);
                    if !second_of_a_double_click {
                        Self::send(tx, TrayCommand::ToggleWindow);
                    }
                }
            }
        }
        let picked = !pending.picks.is_empty();
        for id in &pending.picks {
            if let Some(command) = self.commands.get(id) {
                Self::send(tx, command.clone());
            }
        }
        if pending.settings_changed {
            self.light = (self.taskbar_light)();
        }
        if pending.taskbar_created {
            // What tray-icon added back is what the old taskbar last took, not what is shown now.
            self.shown = Shown::default();
        }
        // A pick: the menu ticked the row it was clicked on, and the mirror says what is ticked.
        let state = shared.take_changed().unwrap_or_else(|| self.state.clone());
        self.draw(icon, state, picked);
    }

    /// Draw `state`: the picture and the tooltip where they differ from what is shown
    /// ([`Tray::show`]), and the menu when it changed — or whatever changed when `rebuild`.
    fn draw(&mut self, icon: &impl Notify, state: TrayState, rebuild: bool) {
        let old = std::mem::replace(&mut self.state, state);
        self.show(icon);
        // Whether audio is flowing changes with every song, and nothing in the menu shows it.
        let menu_of = |state: &TrayState| TrayState {
            processing: false,
            ..state.clone()
        };
        if rebuild || menu_of(&old) != menu_of(&self.state) {
            let (menu, commands) = build_menu(&menu_rows(&self.state));
            icon.menu(menu);
            self.commands = commands;
        }
    }

    /// Bring the picture and the tooltip up to the mirror: each one that differs from what the
    /// notification area shows ([`Shown`]) is set, and kept as shown once the shell has taken it.
    /// One it refuses is set again at the next look, rather than taken for drawn.
    fn show(&mut self, icon: &impl Notify) {
        let picture = (self.state.icon(), self.light);
        if self.shown.picture != Some(picture) {
            self.shown.picture = icon.picture(picture.0, picture.1).then_some(picture);
        }
        let tip = tooltip(&self.state);
        if self.shown.tip.as_ref() != Some(&tip) {
            self.shown.tip = icon.tip(&tip).then_some(tip);
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
    WAKE.set(window.0 as isize);
    // Before the icon exists, which is what sends its events.
    route_events();
    let light = taskbar_is_light();
    let icons = Icons::rasterised(icon_size());
    let (menu, commands) = build_menu(&menu_rows(&state));
    let tip = tooltip(&state);
    let mut builder = TrayIconBuilder::new()
        .with_tooltip(&tip)
        .with_menu(Box::new(menu))
        // The left button toggles the window; the right one opens the menu.
        .with_menu_on_left_click(false);
    let picture = icons.icon(state.icon(), light);
    let pictured = picture.is_some();
    if let Some(image) = picture {
        builder = builder.with_icon(image);
    }
    let icon = match builder.build() {
        Ok(icon) => icon,
        Err(err) => {
            WAKE.set(0);
            // SAFETY: the window made above, on this thread.
            let _ = unsafe { DestroyWindow(window) };
            let _ = ready.send(Err(format!("the tray icon could not be made: {err}")));
            shared.closed.store(true, Ordering::SeqCst);
            return;
        }
    };
    guard_icon_window(HWND(icon.window_handle()));
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
        // What the builder gave tray-icon is what the notification area shows, or will once a
        // taskbar is there to add it to.
        shown: Shown {
            picture: pictured.then_some((state.icon(), light)),
            tip: Some(tip),
        },
        state,
        light,
        taskbar_light: taskbar_is_light,
        commands,
        swallow_release: false,
    };
    let notification = Notification { icon, icons };
    let _ = ready.send(Ok(window.0 as isize));
    let mut msg = MSG::default();
    // SAFETY: the loop of the thread the windows belong to.
    unsafe {
        while GetMessageW(&raw mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
            tray.after_dispatch(&notification, tx, shared);
        }
    }
    shared.closed.store(true, Ordering::SeqCst);
    WAKE.set(0);
    // `NIM_DELETE`, and the icon's own window with it.
    drop(notification);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tray::{TrayDevice, TrayLane, TrayPreset};
    use fxsound_core::WindowsParity;
    use tray_icon::dpi::PhysicalPosition;
    use tray_icon::{Rect, TrayIconId};
    use windows::Win32::UI::WindowsAndMessaging::{
        IsWindow, PM_REMOVE, PeekMessageW, SendMessageW, WM_LBUTTONUP,
    };

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

    // ---- the loop -----------------------------------------------------------------------------

    /// A stand-in for the icon: what it was asked to show, and whether the shell takes it.
    #[derive(Default)]
    struct Recorder {
        /// The shell refuses everything, as it does while Explorer's taskbar is gone.
        refusing: Cell<bool>,
        pictures: RefCell<Vec<(TrayIcon, bool)>>,
        tips: RefCell<Vec<String>>,
        menus: Cell<usize>,
    }

    impl Notify for Recorder {
        fn picture(&self, icon: TrayIcon, light: bool) -> bool {
            self.pictures.borrow_mut().push((icon, light));
            !self.refusing.get()
        }

        fn tip(&self, tip: &str) -> bool {
            self.tips.borrow_mut().push(tip.to_owned());
            !self.refusing.get()
        }

        fn menu(&self, _menu: Menu) {
            self.menus.set(self.menus.get() + 1);
        }
    }

    /// The tray thread's state for `state`, drawn as the builder draws it.
    fn tray(state: TrayState, light: bool, taskbar_light: fn() -> bool) -> Tray {
        Tray {
            shown: Shown {
                picture: Some((state.icon(), light)),
                tip: Some(tooltip(&state)),
            },
            commands: build_menu(&menu_rows(&state)).1,
            state,
            light,
            taskbar_light,
            swallow_release: false,
        }
    }

    /// What the GUI thread shares with the tray thread, with no mirror waiting.
    fn quiet(state: TrayState) -> Shared {
        Shared {
            latest: Mutex::new(state),
            changed: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        }
    }

    /// Run `test` on a thread of its own, as the tray thread is: its own message queue, and its
    /// own [`PENDING`] and [`WAKE`].
    fn on_a_thread_of_its_own(test: impl FnOnce() + Send + 'static) {
        thread::spawn(test).join().expect("the test's thread");
    }

    /// Whether a [`WM_WAKE`] is waiting for `window`; takes it.
    fn woken(window: HWND) -> bool {
        let mut msg = MSG::default();
        // SAFETY: a message of this thread's own queue, into a local.
        unsafe { PeekMessageW(&raw mut msg, Some(window), WM_WAKE, WM_WAKE, PM_REMOVE) }.as_bool()
    }

    fn send(window: HWND, message: u32, lparam: isize) -> LRESULT {
        // SAFETY: a window of this thread's; the procedure runs before the call returns.
        unsafe { SendMessageW(window, message, Some(WPARAM(0)), Some(LPARAM(lparam))) }
    }

    fn left(button_state: MouseButtonState) -> TrayIconEvent {
        TrayIconEvent::Click {
            id: TrayIconId::new("test"),
            position: PhysicalPosition::new(0.0, 0.0),
            rect: Rect::default(),
            button: MouseButton::Left,
            button_state,
        }
    }

    #[test]
    fn a_theme_change_that_windows_sends_wakes_the_loop_and_redraws_the_bars_for_the_taskbar() {
        on_a_thread_of_its_own(|| {
            let window = create_window().expect("the tray's window");
            WAKE.set(window.0 as isize);
            // What `GetMessageW` does with the broadcast, which is never posted: calls the
            // procedure, and goes on waiting.
            send(window, WM_SETTINGCHANGE, 0);
            assert!(woken(window), "a posted message follows the sent one");

            let icon = Recorder::default();
            let state = populated();
            let mut tray = tray(state.clone(), false, || true);
            let (tx, _rx) = crossbeam_channel::unbounded();
            tray.after_dispatch(&icon, &WakingSender::from(tx), &quiet(state));
            assert_eq!(
                *icon.pictures.borrow(),
                [(TrayIcon::On, true)],
                "the bars of on, drawn for the light taskbar at once"
            );
            assert!(icon.tips.borrow().is_empty(), "the tooltip did not change");
            WAKE.set(0);
            // SAFETY: the window made above, on this thread.
            let _ = unsafe { DestroyWindow(window) };
        });
    }

    #[test]
    fn a_click_wakes_the_loop_and_a_double_click_toggles_the_window_once() {
        on_a_thread_of_its_own(|| {
            let window = create_window().expect("the tray's window");
            WAKE.set(window.0 as isize);
            route_events();
            // tray-icon's handler runs inside the shell's callback, which may be sent: each click
            // is followed by a wake-up.
            on_icon_event(left(MouseButtonState::Down));
            assert!(!woken(window), "a press is nothing to act on");
            on_icon_event(left(MouseButtonState::Up));
            assert!(woken(window), "a release is");

            let state = populated();
            let mut tray = tray(state.clone(), false, || false);
            let (tx, rx) = crossbeam_channel::unbounded();
            let tx = WakingSender::from(tx);
            tray.after_dispatch(&Recorder::default(), &tx, &quiet(state.clone()));
            assert_eq!(
                rx.try_iter().collect::<Vec<_>>(),
                [TrayCommand::ToggleWindow]
            );

            // The rest of a double click: the second press and its release, which do not close
            // the window the first one opened.
            on_icon_event(TrayIconEvent::DoubleClick {
                id: TrayIconId::new("test"),
                position: PhysicalPosition::new(0.0, 0.0),
                rect: Rect::default(),
                button: MouseButton::Left,
            });
            on_icon_event(left(MouseButtonState::Up));
            tray.after_dispatch(&Recorder::default(), &tx, &quiet(state));
            assert!(
                rx.try_iter().next().is_none(),
                "one toggle per double click"
            );

            // A pick in the menu goes the same way.
            let exit = tray
                .commands
                .iter()
                .find_map(|(id, command)| (*command == TrayCommand::Exit).then(|| id.clone()))
                .expect("an Exit row");
            on_menu_event(MenuEvent { id: exit });
            assert!(woken(window));
            let icon = Recorder::default();
            tray.after_dispatch(&icon, &tx, &quiet(populated()));
            assert_eq!(rx.try_iter().collect::<Vec<_>>(), [TrayCommand::Exit]);
            assert_eq!(icon.menus.get(), 1, "the menu is rebuilt after a pick");
            WAKE.set(0);
            // SAFETY: the window made above, on this thread.
            let _ = unsafe { DestroyWindow(window) };
        });
    }

    #[test]
    fn a_change_the_shell_refused_is_set_again_and_a_new_taskbar_gets_the_mirror() {
        on_a_thread_of_its_own(|| {
            let icon = Recorder::default();
            let mut tray = tray(populated(), false, || false);
            // Explorer is gone, and FxSound is turned off meanwhile: the shell refuses both.
            icon.refusing.set(true);
            let off = TrayState {
                power: false,
                ..populated()
            };
            tray.draw(&icon, off.clone(), false);
            assert_eq!(*icon.pictures.borrow(), [(TrayIcon::Off, false)]);
            assert_eq!(
                tray.shown,
                Shown::default(),
                "nothing refused is taken for shown"
            );

            // The taskbar is back: tray-icon added the icon from what the old taskbar last took,
            // the On icon, and the loop hears of it after tray-icon has.
            icon.refusing.set(false);
            PENDING.with_borrow_mut(|pending| pending.taskbar_created = true);
            let (tx, _rx) = crossbeam_channel::unbounded();
            tray.after_dispatch(&icon, &WakingSender::from(tx), &quiet(off.clone()));
            assert_eq!(
                *icon.pictures.borrow(),
                [(TrayIcon::Off, false), (TrayIcon::Off, false)],
                "the mirror's icon over the one tray-icon kept"
            );
            assert_eq!(icon.tips.borrow().last(), Some(&tooltip(&off)));
            assert_eq!(
                tray.shown,
                Shown {
                    picture: Some((TrayIcon::Off, false)),
                    tip: Some(tooltip(&off)),
                }
            );

            // Shown, it is not set again for nothing.
            let (tx, _rx) = crossbeam_channel::unbounded();
            tray.after_dispatch(&icon, &WakingSender::from(tx), &quiet(off));
            assert_eq!(icon.pictures.borrow().len(), 2);
            assert_eq!(icon.tips.borrow().len(), 2);
        });
    }

    thread_local! {
        /// What the stand-in for tray-icon's window got: the message, its `lParam`, and whether
        /// the loop had been told of a new taskbar by then.
        static GOT: RefCell<Vec<(u32, isize, bool)>> = const { RefCell::new(Vec::new()) };
    }

    /// A stand-in for tray-icon's window: it notes what reaches it, and leaves the rest to
    /// `DefWindowProcW`, as tray-icon does — `WM_CLOSE` included.
    fn stand_in_for_tray_icons_window() -> HWND {
        unsafe extern "system" fn proc(
            window: HWND,
            message: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            if message == TRAY_ICON_CALLBACK
                || message == WM_CLOSE
                || Some(message) == taskbar_created()
            {
                let told = PENDING.with_borrow(|pending| pending.taskbar_created);
                GOT.with_borrow_mut(|got| got.push((message, lparam.0, told)));
            }
            // SAFETY: the window's own procedure.
            unsafe { DefWindowProcW(window, message, wparam, lparam) }
        }
        static REGISTERED: OnceLock<()> = OnceLock::new();
        let name = w!("FxSoundTrayTestIconWindow");
        // SAFETY: this module's own handle.
        let instance = unsafe { GetModuleHandleW(None) }.ok().map(Into::into);
        REGISTERED.get_or_init(|| {
            let class = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: instance.unwrap_or_default(),
                lpszClassName: name,
                ..WNDCLASSW::default()
            };
            // SAFETY: `class` outlives the call.
            unsafe { RegisterClassW(&raw const class) };
        });
        // SAFETY: the class is registered; the window is this thread's and never shown.
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0),
                name,
                PCWSTR::null(),
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
        .expect("the stand-in's window")
    }

    #[test]
    fn the_icons_window_keeps_the_moves_and_the_close_and_tells_of_a_new_taskbar_after_tray_icon() {
        on_a_thread_of_its_own(|| {
            let window = stand_in_for_tray_icons_window();
            WAKE.set(window.0 as isize);
            guard_icon_window(window);

            send(window, TRAY_ICON_CALLBACK, WM_MOUSEMOVE as isize);
            send(window, TRAY_ICON_CALLBACK, WM_LBUTTONUP as isize);
            assert_eq!(
                GOT.take(),
                [(TRAY_ICON_CALLBACK, WM_LBUTTONUP as isize, false)],
                "the click reaches tray-icon, the move that would start its hover timer does not"
            );

            send(window, WM_CLOSE, 0);
            assert!(
                GOT.take().is_empty(),
                "the close never reaches DefWindowProcW"
            );
            // SAFETY: a question about a window of this thread's.
            assert!(
                unsafe { IsWindow(Some(window)) }.as_bool(),
                "and the icon's window stays"
            );

            let created = taskbar_created().expect("TaskbarCreated is registered");
            send(window, created, 0);
            assert_eq!(
                GOT.take(),
                [(created, 0, false)],
                "tray-icon adds the icon back before the loop is told"
            );
            assert!(PENDING.with_borrow(|pending| pending.taskbar_created));
            assert!(woken(window));

            WAKE.set(0);
            // SAFETY: the window made above, on this thread; the subclass takes itself away.
            unsafe { DestroyWindow(window) }.expect("destroyed");
        });
    }
}
