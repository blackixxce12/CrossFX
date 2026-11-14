//! Every call the Windows backend makes into Windows: COM and its apartments, the MMDevice API, the
//! audio clients, the endpoint volume, MMCSS, events, the default-device policy and the process
//! list — each behind a safe wrapper, so that nothing outside this file is `unsafe`.
//!
//! This is the one module of the crate besides `aec` that may use `unsafe`
//! (`#[allow(unsafe_code)]` where it is declared; `lib.rs` says why the crate denies rather than
//! forbids it). Every block says why it is sound. Most are the `windows` crate's own wrappers,
//! which are `unsafe` because a COM call is: the object must be alive, the pointers handed over
//! valid for the call. The objects here are owned by the wrappers that call them, so they are alive
//! for every call, and the pointers are locals or buffers WASAPI itself handed out.
//!
//! # IPolicyConfig
//!
//! The one undocumented interface. Making an endpoint the system's default has no public API:
//! the sound settings do it through `IPolicyConfig`, implemented by the `PolicyConfigClient` class
//! in `AudioSes.dll`, and so does every tool that switches the default device — the original's
//! `sndDevicesSetDeviceType` (`docs/spec/12-audio-io.md` §6), EarTrumpet, SoundSwitch. The original
//! asked for its Vista-era twin (`IPolicyConfigVista`, `568b9108-…`); this asks for the interface
//! Windows 7 and every Windows since implement. Checked against three public declarations, which
//! agree on all of it:
//!
//! * EarTrumpet, `EarTrumpet/Interop/MMDeviceAPI/IPolicyConfig.cs` (`IPolicyConfigWin7`, marked
//!   "Win7-Win8, W10_RS1-Present") and `PolicyConfigClient.cs`;
//! * the `PolicyConfig.h` LizardByte's Sunshine ships — EreTIk's widely copied declaration, whose
//!   Windows 7 GUIDs are the ones the original's own `PolicyConfig.h:23-26` carries;
//! * the `com-policy-config` crate (sidit77, MIT), a Rust port of the same header.
//!
//! So: class `870AF99C-171D-4F9E-AF0D-E63DF40C2BC9`, interface
//! `F8679F50-850A-41CF-9C72-430F290290C8`, and twelve methods after `IUnknown`'s in this order —
//! `GetMixFormat`, `GetDeviceFormat`, `ResetDeviceFormat`, `SetDeviceFormat`,
//! `GetProcessingPeriod`, `SetProcessingPeriod`, `GetShareMode`, `SetShareMode`,
//! `GetPropertyValue`, `SetPropertyValue`, `SetDefaultEndpoint(PCWSTR, ERole)`,
//! `SetEndpointVisibility` — which puts `SetDefaultEndpoint` in slot 13 of the vtable (EarTrumpet
//! declares the first eight as `Unused1`…`Unused8`).
//!
//! Only two methods are ever called: `SetDefaultEndpoint`, and `GetMixFormat` in the live check
//! that the class and the layout are right (`live`), which reads an endpoint's mix format through
//! the interface and holds it to what `IAudioClient::GetMixFormat` says; the size of the vtable is
//! asserted where the interface is declared. The property methods are declared with the
//! parameters EarTrumpet and the header give them; the crate gives both a `bFxStore` flag besides.
//! Neither is called, and a slot's signature does not move the slots after it.

use std::time::Duration;

use crossbeam_channel::Sender;
use fxsound_core::DeviceDirection;
use windows::Win32::Devices::FunctionDiscovery::{
    PKEY_Device_ContainerId, PKEY_Device_DeviceDesc, PKEY_Device_EnumeratorName,
    PKEY_Device_FriendlyName, PKEY_DeviceInterface_FriendlyName,
};
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, PROPERTYKEY, RPC_E_CHANGED_MODE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SESSIONFLAGS_DISPLAY_HIDE, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
    AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE, DEVICE_STATE_ACTIVE, EDataFlow, ERole,
    IAudioCaptureClient, IAudioClient, IAudioRenderClient, IAudioSessionControl,
    IAudioSessionControl2, IAudioSessionManager2, IAudioSessionNotification,
    IAudioSessionNotification_Impl, IMMDevice, IMMDeviceEnumerator, IMMNotificationClient,
    IMMNotificationClient_Impl, MMDeviceEnumerator, PKEY_AudioEndpoint_FormFactor, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE, eCapture, eCommunications, eConsole, eMultimedia, eRender,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PropVariantToGUID, PropVariantToStringAlloc, PropVariantToUInt32,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize, STGM_READ,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW,
    GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW, SetEvent, WaitForMultipleObjects,
    WaitForSingleObject,
};
use windows_core::{HRESULT, Interface, PCWSTR, PWSTR};

use super::Event;
use super::defaults::{Defaults, Role};
use super::endpoints::Endpoint;
use super::format::{MixFormat, StreamFormat, WAVE_FORMAT_EXTENSIBLE};
use super::original::{self, Process};

/// `ERROR_NOT_FOUND` as an `HRESULT`, what `IMMDeviceEnumerator::GetDevice` and
/// `GetDefaultAudioEndpoint` answer for an endpoint that is not there.
pub(crate) const E_NOTFOUND: HRESULT = HRESULT(0x8007_0490_u32 as i32);

/// A NUL-terminated UTF-16 copy of `text`, for a `PCWSTR` parameter.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Take a string COM allocated and handed over (`IMMDevice::GetId`, `PropVariantToStringAlloc`),
/// and free it.
fn take_string(text: PWSTR) -> String {
    if text.is_null() {
        return String::new();
    }
    // SAFETY: `text` is a NUL-terminated string the callee allocated with `CoTaskMemAlloc` and
    // handed to us; it is read once here and freed once, after the read, and not used again.
    unsafe {
        let read = text.to_string().unwrap_or_default();
        CoTaskMemFree(Some(text.as_ptr().cast_const().cast()));
        read
    }
}

/// The data flow of a direction: render endpoints are outputs, capture endpoints inputs.
const fn flow(direction: DeviceDirection) -> EDataFlow {
    match direction {
        DeviceDirection::Output => eRender,
        DeviceDirection::Input => eCapture,
    }
}

const fn role(role: Role) -> ERole {
    match role {
        Role::Console => eConsole,
        Role::Multimedia => eMultimedia,
        Role::Communications => eCommunications,
    }
}

// ---------------------------------------------------------------------------------------------
// Threads: the apartment, MMCSS, events
// ---------------------------------------------------------------------------------------------

/// This thread's membership of the multithreaded apartment, for as long as the value lives. Every
/// thread of the backend joins it before it touches COM and leaves it last, after every object it
/// made: declared first, it is dropped last. Not `Send`: leaving the apartment is the business of
/// the thread that joined it.
pub(crate) struct Apartment(std::marker::PhantomData<*const ()>);

impl Apartment {
    /// Join the MTA. The MMDevice API and WASAPI work from any apartment, but the notification
    /// callbacks arrive on threads of the system's own, so nothing here may depend on a message
    /// loop, which an STA would need.
    ///
    /// # Errors
    /// The `HRESULT` of a thread that is in a single-threaded apartment already, or of COM
    /// failing outright.
    #[allow(unsafe_code)]
    pub(crate) fn join() -> windows_core::Result<Self> {
        // SAFETY: no reserved pointer; called once per thread, by the thread that drops the value.
        let joined = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if joined == RPC_E_CHANGED_MODE {
            return Err(joined.into());
        }
        // `S_FALSE` — the thread was in the MTA already — counts too, and needs its uninitialize.
        joined.ok().map(|()| Self(std::marker::PhantomData))
    }
}

impl Drop for Apartment {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: balances the successful `CoInitializeEx` of `join`, on the thread that called it
        // (the value is not `Send`), after the objects declared after it have been released.
        unsafe { CoUninitialize() };
    }
}

/// This thread registered with MMCSS as "Pro Audio" for as long as the value lives: the Windows
/// twin of what RTKit does for PipeWire's data thread (`docs/spec/12-audio-io.md` §23), and the
/// successor of the original's `THREAD_PRIORITY_TIME_CRITICAL` (`AudioPassthruPrivate.cpp:493`).
pub(crate) struct ProAudio(HANDLE);

impl ProAudio {
    /// Register the calling thread. `None`, and the stream runs at normal priority, when MMCSS
    /// will not have it (the service stopped, a policy) — a stream that may glitch under load is
    /// better than none.
    #[allow(unsafe_code)]
    pub(crate) fn join() -> Option<Self> {
        let task = wide("Pro Audio");
        let mut index = 0_u32;
        // SAFETY: `task` is a NUL-terminated string that outlives the call; `index` is a local.
        match unsafe { AvSetMmThreadCharacteristicsW(PCWSTR(task.as_ptr()), &raw mut index) } {
            Ok(handle) => Some(Self(handle)),
            Err(error) => {
                log::warn!("MMCSS would not take the audio thread: {error}");
                None
            }
        }
    }
}

impl Drop for ProAudio {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: the handle `AvSetMmThreadCharacteristicsW` gave this thread, reverted once, by
        // the thread it was given to (the value is a local of the thread that made it).
        if let Err(error) = unsafe { AvRevertMmThreadCharacteristics(self.0) } {
            log::debug!("MMCSS: {error}");
        }
    }
}

/// An auto-reset event: what WASAPI signals when a stream has a buffer ready, and what the control
/// thread signals to wake a stream thread out of its wait ([`Self::set`]).
#[derive(Debug)]
pub(crate) struct WaitEvent(HANDLE);

// SAFETY: an event handle is a kernel object's, valid in every thread of the process: waiting on
// it and setting it are thread-safe calls, and the handle is closed once, by `Drop`, when the last
// owner lets go of the value.
#[allow(unsafe_code)]
unsafe impl Send for WaitEvent {}
// SAFETY: as above; `&WaitEvent` only waits on the event or sets it.
#[allow(unsafe_code)]
unsafe impl Sync for WaitEvent {}

impl WaitEvent {
    #[allow(unsafe_code)]
    pub(crate) fn new() -> windows_core::Result<Self> {
        // SAFETY: default security, auto-reset, unsignalled, unnamed: no pointer is handed over.
        unsafe { CreateEventW(None, false, false, PCWSTR::null()) }.map(Self)
    }

    /// Signal the event: a thread waiting on it wakes, or the next one to wait returns at once.
    #[allow(unsafe_code)]
    pub(crate) fn set(&self) {
        // SAFETY: the handle `CreateEventW` returned, alive until `Drop`.
        if let Err(error) = unsafe { SetEvent(self.0) } {
            log::debug!("setting an event: {error}");
        }
    }
}

impl Drop for WaitEvent {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: the handle `CreateEventW` returned, closed once. An audio client it was handed
        // to (`SetEventHandle`) is released before this on every path: by the order of the fields
        // of the stream that owns both once it is built, and on the open functions' early returns
        // by the order of their locals — the event is made before the client, and locals drop in
        // the reverse of the order they were made in.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// What a wait for several events ended with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Woken {
    /// One of them was signalled.
    Signalled,
    /// None was, in time.
    TimedOut,
    /// The wait itself failed.
    Failed,
}

/// Wait up to `timeout` for any of `events`. With none, a plain sleep.
#[allow(unsafe_code)]
pub(crate) fn wait_any(events: &[&WaitEvent], timeout: Duration) -> Woken {
    let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
    if events.is_empty() {
        std::thread::sleep(timeout);
        return Woken::TimedOut;
    }
    let mut handles = [HANDLE::default(); 4];
    let count = events.len().min(handles.len());
    for (slot, event) in handles.iter_mut().zip(events) {
        *slot = event.0;
    }
    // SAFETY: every handle is an event this thread owns and keeps alive for the wait.
    let woken = unsafe { WaitForMultipleObjects(&handles[..count], false, millis) };
    if woken == WAIT_TIMEOUT {
        Woken::TimedOut
    } else if woken
        .0
        .checked_sub(WAIT_OBJECT_0.0)
        .is_some_and(|index| (index as usize) < count)
    {
        Woken::Signalled
    } else {
        Woken::Failed
    }
}

// ---------------------------------------------------------------------------------------------
// The MMDevice API
// ---------------------------------------------------------------------------------------------

/// The endpoint enumerator, and the place notifications are registered with.
pub(crate) struct Enumerator(IMMDeviceEnumerator);

impl Enumerator {
    /// # Errors
    /// When the MMDevice API cannot be reached: no audio service, or COM not joined.
    #[allow(unsafe_code)]
    pub(crate) fn new() -> windows_core::Result<Self> {
        // SAFETY: a documented class, no aggregation; the calling thread is in the MTA.
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map(Self)
    }

    /// Every active endpoint of `direction`, each read from its property store. An endpoint whose
    /// store cannot be read is left out — it is going, or never finished arriving — rather than
    /// failing the list.
    ///
    /// # Errors
    /// When the enumeration itself fails.
    #[allow(unsafe_code)]
    pub(crate) fn endpoints(
        &self,
        direction: DeviceDirection,
    ) -> windows_core::Result<Vec<Endpoint>> {
        // SAFETY: the enumerator is alive (owned by `self`); the collection it returns is owned
        // by this frame.
        let collection = unsafe {
            self.0
                .EnumAudioEndpoints(flow(direction), DEVICE_STATE_ACTIVE)
        }?;
        // SAFETY: as above.
        let count = unsafe { collection.GetCount() }?;
        let mut endpoints = Vec::with_capacity(count as usize);
        for index in 0..count {
            // SAFETY: `index` is below the count the same collection reported.
            let Ok(device) = (unsafe { collection.Item(index) }) else {
                continue;
            };
            match Device(device).describe(direction) {
                Ok(endpoint) => endpoints.push(endpoint),
                Err(error) => {
                    log::debug!("skipping an endpoint whose store cannot be read: {error}")
                }
            }
        }
        Ok(endpoints)
    }

    /// The default endpoint of `direction` for `role`, by ID; `None` when there is none.
    #[allow(unsafe_code)]
    pub(crate) fn default_id(&self, direction: DeviceDirection, role: Role) -> Option<String> {
        // SAFETY: the enumerator is alive; the device returned is owned by this frame.
        let device = unsafe {
            self.0
                .GetDefaultAudioEndpoint(flow(direction), self::role(role))
        }
        .ok()?;
        Device(device).id().ok()
    }

    /// Every role's default endpoint of `direction`.
    pub(crate) fn defaults(&self, direction: DeviceDirection) -> Defaults {
        let mut defaults = Defaults::default();
        for role in Role::ALL {
            defaults.set(role, self.default_id(direction, role));
        }
        defaults
    }

    /// The endpoint called `id`.
    ///
    /// # Errors
    /// [`E_NOTFOUND`] for an endpoint that is not there.
    #[allow(unsafe_code)]
    pub(crate) fn device(&self, id: &str) -> windows_core::Result<Device> {
        let id = wide(id);
        // SAFETY: `id` is NUL-terminated and outlives the call.
        unsafe { self.0.GetDevice(PCWSTR(id.as_ptr())) }.map(Device)
    }

    /// Have every change of the endpoints and of the defaults sent to `events` until the returned
    /// value is dropped.
    ///
    /// # Errors
    /// When the registration is refused.
    #[allow(unsafe_code)]
    pub(crate) fn watch(&self, events: Sender<Event>) -> windows_core::Result<Watch> {
        let client: IMMNotificationClient = Notifier { events }.into();
        // SAFETY: the enumerator and the client are alive; the enumerator keeps a reference to the
        // client until it is unregistered, which `Watch::drop` does before either is released.
        unsafe { self.0.RegisterEndpointNotificationCallback(&client) }?;
        Ok(Watch {
            enumerator: self.0.clone(),
            client,
        })
    }
}

/// A registration of [`Notifier`] with the enumerator; unregistered when dropped.
pub(crate) struct Watch {
    enumerator: IMMDeviceEnumerator,
    client: IMMNotificationClient,
}

impl Drop for Watch {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: both are alive (owned by `self`), and this is the client registered in
        // `Enumerator::watch`. Once this returns no callback is running or will run.
        if let Err(error) = unsafe {
            self.enumerator
                .UnregisterEndpointNotificationCallback(&self.client)
        } {
            log::debug!("unregistering the endpoint notifications: {error}");
        }
    }
}

/// The notification client: every endpoint change and every default change, as an [`Event`] on
/// the control thread's channel.
///
/// Its methods run on threads of the system's, and must not block or call back into the
/// enumerator (the original's comments record the deadlocks that come of it): each one only sends,
/// on an unbounded channel, which never blocks. Endpoints that come and go are one event however
/// many of them there are — a USB headset is a dozen state changes (`sndDevicesDeviceCallbacks.cpp:225`)
/// — and the control thread reads the endpoints afresh once the burst is over. Property changes
/// are not listened to, as the original does not (`:271-309`): they arrive for every volume and
/// mute change.
#[windows_core::implement(IMMNotificationClient)]
struct Notifier {
    events: Sender<Event>,
}

impl IMMNotificationClient_Impl for Notifier_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> windows_core::Result<()> {
        let _ = self.events.send(Event::Devices);
        Ok(())
    }

    fn OnDeviceAdded(&self, _: &PCWSTR) -> windows_core::Result<()> {
        let _ = self.events.send(Event::Devices);
        Ok(())
    }

    fn OnDeviceRemoved(&self, _: &PCWSTR) -> windows_core::Result<()> {
        let _ = self.events.send(Event::Devices);
        Ok(())
    }

    #[allow(unsafe_code)]
    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        role: ERole,
        id: &PCWSTR,
    ) -> windows_core::Result<()> {
        let direction = match flow {
            f if f == eRender => DeviceDirection::Output,
            f if f == eCapture => DeviceDirection::Input,
            _ => return Ok(()),
        };
        let role = match role {
            r if r == eConsole => Role::Console,
            r if r == eMultimedia => Role::Multimedia,
            r if r == eCommunications => Role::Communications,
            _ => return Ok(()),
        };
        let id = if id.is_null() {
            None
        } else {
            // SAFETY: a non-null `pwstrDefaultDeviceId` is a NUL-terminated string Windows keeps
            // alive for the duration of the call; it is copied here and not kept.
            unsafe { id.to_string() }.ok()
        };
        let _ = self.events.send(Event::DefaultChanged {
            direction,
            role,
            id,
        });
        Ok(())
    }

    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> windows_core::Result<()> {
        Ok(())
    }
}

/// One endpoint.
pub(crate) struct Device(IMMDevice);

impl Device {
    /// The endpoint ID (`IMMDevice::GetId`).
    #[allow(unsafe_code)]
    pub(crate) fn id(&self) -> windows_core::Result<String> {
        // SAFETY: the device is alive; the string is ours to free, which `take_string` does.
        unsafe { self.0.GetId() }.map(take_string)
    }

    /// The endpoint as [`Endpoint`] describes it: its ID and what its property store says.
    #[allow(unsafe_code)]
    fn describe(&self, direction: DeviceDirection) -> windows_core::Result<Endpoint> {
        let id = self.id()?;
        // SAFETY: the device is alive; the store is owned by this frame.
        let store = unsafe { self.0.OpenPropertyStore(STGM_READ) }?;
        // The `PROPVARIANT` the store returns owns what it holds and clears itself when dropped
        // (the `windows` crate's own `Drop`, which calls `PropVariantClear`): each one below is
        // read through a pointer that does not outlive it, and cleared once, by that `Drop`.
        let read = |key: &PROPERTYKEY| -> Option<PROPVARIANT> {
            // SAFETY: the store is alive and `key` is a static key; the variant returned is ours.
            unsafe { store.GetValue(key) }.ok()
        };
        let text = |key: &PROPERTYKEY| -> String {
            read(key)
                .and_then(|value| {
                    // SAFETY: `value` is a live variant; the string allocated is ours to free,
                    // which `take_string` does. An empty variant reads as an error, and as "".
                    unsafe { PropVariantToStringAlloc(&raw const value) }.ok()
                })
                .map(take_string)
                .unwrap_or_default()
        };
        let form_factor = read(&PKEY_AudioEndpoint_FormFactor).and_then(|value| {
            // SAFETY: `value` is a live variant, read and not kept.
            unsafe { PropVariantToUInt32(&raw const value) }.ok()
        });
        let container = read(&PKEY_Device_ContainerId).and_then(|value| {
            // SAFETY: as above.
            unsafe { PropVariantToGUID(&raw const value) }
                .ok()
                .map(|guid| format!("{guid:?}"))
        });
        Ok(Endpoint {
            id,
            direction,
            friendly_name: text(&PKEY_Device_FriendlyName),
            description: text(&PKEY_Device_DeviceDesc),
            interface_name: text(&PKEY_DeviceInterface_FriendlyName),
            enumerator: text(&PKEY_Device_EnumeratorName),
            container,
            form_factor,
        })
    }

    /// A fresh, uninitialised audio client on the endpoint. WASAPI forbids initialising one
    /// twice, so every attempt at a stream takes a new one (`sndDevicesSetupDevices.cpp:425-431`).
    #[allow(unsafe_code)]
    fn client(&self) -> windows_core::Result<IAudioClient> {
        // SAFETY: the device is alive; no activation parameters.
        unsafe { self.0.Activate::<IAudioClient>(CLSCTX_ALL, None) }
    }

    /// The endpoint's shared-mode mix format, from a throw-away audio client
    /// (`sndDevicesGet.cpp:431-459`).
    ///
    /// # Errors
    /// When the endpoint will not give a client, or the client no format.
    pub(crate) fn mix_format(&self) -> windows_core::Result<MixFormat> {
        MixFormatPtr::of(&self.client()?).map(|format| format.read())
    }

    /// The endpoint's volume as it stands — the slider's position, its level in decibels, the
    /// mute — and which of volume, mute and meter the device does itself
    /// (`ENDPOINT_HARDWARE_SUPPORT_*`). Read, and nothing registered: for the probe.
    ///
    /// # Errors
    /// When the endpoint has no volume control.
    #[allow(unsafe_code)]
    pub(crate) fn volume_levels(&self) -> windows_core::Result<(f32, f32, bool, u32)> {
        // SAFETY: the device is alive; the control is owned by this frame and only read.
        unsafe {
            let volume = self.0.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)?;
            Ok((
                volume.GetMasterVolumeLevelScalar()?,
                volume.GetMasterVolumeLevel()?,
                volume.GetMute()?.as_bool(),
                volume.QueryHardwareSupport()?,
            ))
        }
    }

    /// The endpoint's own volume, watched: every change sent to `events` as
    /// [`Event::Volume`] for `direction`, until the returned value is dropped.
    ///
    /// # Errors
    /// When the endpoint has no volume control, or refuses the registration.
    #[allow(unsafe_code)]
    pub(crate) fn volume(
        &self,
        events: Sender<Event>,
        direction: DeviceDirection,
    ) -> windows_core::Result<EndpointVolume> {
        // SAFETY: the device is alive; no activation parameters.
        let volume = unsafe { self.0.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) }?;
        let callback: IAudioEndpointVolumeCallback = VolumeNotifier { events, direction }.into();
        // SAFETY: both are alive; the endpoint keeps a reference to the callback until it is
        // unregistered, which `EndpointVolume::drop` does first.
        unsafe { volume.RegisterControlChangeNotify(&callback) }?;
        Ok(EndpointVolume { volume, callback })
    }
}

/// A mix format as `IAudioClient::GetMixFormat` allocated it, freed when dropped. Kept, rather
/// than read and freed at once, so that a stream can be opened with the very structure the
/// endpoint handed out: a copy of it is refused by some drivers (`sndDevicesSetupDevices.cpp:450`).
struct MixFormatPtr(*mut WAVEFORMATEX);

impl MixFormatPtr {
    #[allow(unsafe_code)]
    fn of(client: &IAudioClient) -> windows_core::Result<Self> {
        // SAFETY: the client is alive; the format is ours to free, which `drop` does.
        let format = unsafe { client.GetMixFormat() }?;
        if format.is_null() {
            return Err(windows::Win32::Foundation::E_POINTER.into());
        }
        Ok(Self(format))
    }

    /// What the format says.
    #[allow(unsafe_code)]
    fn read(&self) -> MixFormat {
        // SAFETY: the pointer is non-null and points at the format GetMixFormat allocated, at
        // least a `WAVEFORMATEX` long and, when its tag and `cbSize` say so, a whole
        // `WAVEFORMATEXTENSIBLE`; both are `packed(1)`, read unaligned.
        unsafe {
            let head = self.0.read_unaligned();
            let extensible = head.wFormatTag == WAVE_FORMAT_EXTENSIBLE
                && usize::from(head.cbSize)
                    >= size_of::<WAVEFORMATEXTENSIBLE>() - size_of::<WAVEFORMATEX>();
            if extensible {
                MixFormat::from_extensible(&self.0.cast::<WAVEFORMATEXTENSIBLE>().read_unaligned())
            } else {
                MixFormat::from_ex(&head)
            }
        }
    }
}

impl Drop for MixFormatPtr {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: the `CoTaskMemAlloc`ed format `of` received, freed once.
        unsafe { CoTaskMemFree(Some(self.0.cast_const().cast())) };
    }
}

// ---------------------------------------------------------------------------------------------
// The endpoint volume
// ---------------------------------------------------------------------------------------------

/// An endpoint's volume control, watched (`Device::volume`).
pub(crate) struct EndpointVolume {
    volume: IAudioEndpointVolume,
    callback: IAudioEndpointVolumeCallback,
}

impl EndpointVolume {
    /// The gain the endpoint's volume asks for, as an amplitude: `0.0` when it is muted or at the
    /// bottom of its slider, and otherwise its level in decibels — the curve its own driver
    /// gives the slider — as an amplitude up to unity.
    ///
    /// # Errors
    /// When the endpoint will not say.
    #[allow(unsafe_code)]
    pub(crate) fn gain(&self) -> windows_core::Result<f32> {
        // SAFETY: the control is alive (owned by `self`); each call returns a value.
        let (muted, scalar, decibels) = unsafe {
            (
                self.volume.GetMute()?.as_bool(),
                self.volume.GetMasterVolumeLevelScalar()?,
                self.volume.GetMasterVolumeLevel()?,
            )
        };
        Ok(gain_of(muted, scalar, decibels))
    }
}

impl Drop for EndpointVolume {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: both alive; this is the callback `Device::volume` registered.
        if let Err(error) = unsafe { self.volume.UnregisterControlChangeNotify(&self.callback) } {
            log::debug!("unregistering the volume notifications: {error}");
        }
    }
}

/// The gain of a volume that is `muted` or not, its slider at `scalar` and its level `decibels`.
///
/// The loopback capture the output lane plays is tapped before the virtual device's volume and
/// mute (`AUDCLNT_STREAMOPTIONS_POST_VOLUME_LOOPBACK`'s documentation: "the default behavior is
/// for the loopback stream to be tapped before volume and/or mute"), and FxSound's own driver
/// keeps its volume in hardware besides (`ENDPOINT_HARDWARE_SUPPORT_VOLUME`), where nothing
/// applies it to the sound at all. So the volume keys, which move the default device's volume —
/// the virtual device's — would change nothing the user hears unless the lane applied it: it does,
/// after the chain, as the Linux lanes apply their node's volume. The level in decibels, not the
/// slider's position, is what a device with that volume would play at.
pub(crate) fn gain_of(muted: bool, scalar: f32, decibels: f32) -> f32 {
    if muted || scalar.is_nan() || scalar <= 0.0 || !decibels.is_finite() {
        return 0.0;
    }
    10.0_f32.powf(decibels.min(0.0) / 20.0).clamp(0.0, 1.0)
}

/// The volume callback: like [`Notifier`], it only sends.
#[windows_core::implement(IAudioEndpointVolumeCallback)]
struct VolumeNotifier {
    events: Sender<Event>,
    direction: DeviceDirection,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeNotifier_Impl {
    fn OnNotify(&self, _: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows_core::Result<()> {
        let _ = self.events.send(Event::Volume(self.direction));
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Audio sessions
// ---------------------------------------------------------------------------------------------

/// An endpoint's audio sessions, watched (`Device::watch_sessions`): every session another process
/// opens there is sent to the control thread as [`Event::Session`] — the original FxSound's
/// loopback capture of its driver above all, which shows that it has started. Unregistered when
/// dropped.
pub(crate) struct SessionWatch {
    manager: IAudioSessionManager2,
    client: IAudioSessionNotification,
}

impl Drop for SessionWatch {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: both are alive (owned by `self`), and this is the client `watch_sessions`
        // registered. Dropped on the control thread, never from inside the callback.
        if let Err(error) = unsafe { self.manager.UnregisterSessionNotification(&self.client) } {
            log::debug!("unregistering the session notifications: {error}");
        }
    }
}

/// The session callback: like [`Notifier`], it only sends — and only for a session of another
/// process's, so that the lanes' own streams wake nothing.
#[windows_core::implement(IAudioSessionNotification)]
struct SessionNotifier {
    events: Sender<Event>,
    endpoint: String,
    own_pid: u32,
}

impl IAudioSessionNotification_Impl for SessionNotifier_Impl {
    #[allow(unsafe_code)]
    fn OnSessionCreated(
        &self,
        session: windows_core::Ref<IAudioSessionControl>,
    ) -> windows_core::Result<()> {
        let pid = session
            .as_ref()
            .and_then(|session| session.cast::<IAudioSessionControl2>().ok())
            // SAFETY: the session Windows hands over is alive for the call; read and not kept.
            .and_then(|session| unsafe { session.GetProcessId() }.ok());
        if pid != Some(self.own_pid) {
            let _ = self.events.send(Event::Session(self.endpoint.clone()));
        }
        Ok(())
    }
}

impl Device {
    /// The endpoint's audio sessions, watched ([`SessionWatch`]) under the name `endpoint`.
    ///
    /// # Errors
    /// When the endpoint has no session manager, or refuses the registration.
    #[allow(unsafe_code)]
    pub(crate) fn watch_sessions(
        &self,
        events: Sender<Event>,
        endpoint: &str,
    ) -> windows_core::Result<SessionWatch> {
        // SAFETY: the device is alive; no activation parameters.
        let manager = unsafe { self.0.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) }?;
        // The session manager tells of no session created until its sessions have been listed
        // once: a documented quirk of `RegisterSessionNotification`.
        // SAFETY: the manager is alive; the enumerator is dropped at once.
        drop(unsafe { manager.GetSessionEnumerator() }?);
        let client: IAudioSessionNotification = SessionNotifier {
            events,
            endpoint: endpoint.to_owned(),
            own_pid: own_pid(),
        }
        .into();
        // SAFETY: both are alive; the manager keeps a reference to the client until it is
        // unregistered, which `SessionWatch::drop` does.
        unsafe { manager.RegisterSessionNotification(&client) }?;
        Ok(SessionWatch { manager, client })
    }
}

// ---------------------------------------------------------------------------------------------
// Streams
// ---------------------------------------------------------------------------------------------

/// How much the capture buffer holds: a fifth of a second, far more than a packet. It costs no
/// latency — packets are taken as they arrive — and lets a thread that was held up a moment find
/// what arrived meanwhile.
const CAPTURE_BUFFER_HNS: i64 = 2_000_000;

/// How much the render buffer may hold: a tenth of a second. The lane keeps it holding far less
/// (`super::ring::Targets::padding`); the rest is room.
const RENDER_BUFFER_HNS: i64 = 1_000_000;

/// The session flag every stream of a lane is opened with: its session is left out of the volume
/// mixer, as the original left its own out (`sndDevicesSetupDevices.cpp:169-173`, `:444-448`).
/// Listed, FxSound's render session would be a second slider beside the speakers' own, and one the
/// user muted there — which Windows then keeps for the application, run after run — would silence
/// everything FxSound plays with no FxSound control to say why. The volume the user hears through
/// FxSound is the virtual device's, the one the volume keys move.
const HIDDEN: u32 = AUDCLNT_SESSIONFLAGS_DISPLAY_HIDE;

/// What a capture stream records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// What applications play into a playback endpoint (`AUDCLNT_STREAMFLAGS_LOOPBACK`): the
    /// output lane, on the virtual device, at that device's own format.
    Loopback,
    /// A microphone: the input lane, at 48 kHz and a stereo pair at least.
    Microphone,
}

/// A shared-mode capture stream, started and stopped by its owner.
pub(crate) struct CaptureStream {
    // Field order is drop order: the client goes before the event it was given.
    capture: IAudioCaptureClient,
    client: IAudioClient,
    event: Option<WaitEvent>,
    format: StreamFormat,
    period: usize,
    /// The rate the endpoint itself runs at: its mix format's.
    native_rate: u32,
}

/// One packet the capture stream had for the lane.
pub(crate) enum Packet<'a> {
    /// Samples: little-endian `f32`, interleaved, whole frames.
    Samples(&'a [u8]),
    /// So many frames of silence (`AUDCLNT_BUFFERFLAGS_SILENT`): the buffer holds nothing to read.
    Silence(usize),
}

impl CaptureStream {
    /// Open a capture stream of `source` on `device`.
    ///
    /// The stream is event-driven when the endpoint allows it — a loopback stream signals its
    /// packets from Windows 10 on, a microphone always — and polled otherwise: the lane's thread
    /// wakes for the render stream's events in any case, and reads whatever arrived.
    ///
    /// # Errors
    /// The `HRESULT` the endpoint refused the stream with, for the caller to name.
    #[allow(unsafe_code)]
    pub(crate) fn open(device: &Device, source: Source) -> windows_core::Result<Self> {
        let mix = MixFormatPtr::of(&device.client()?)?;
        let mix_format = mix.read();
        let format = match source {
            Source::Loopback => StreamFormat::for_loopback(&mix_format)
                .ok_or(windows::Win32::Media::Audio::AUDCLNT_E_UNSUPPORTED_FORMAT)?,
            Source::Microphone => StreamFormat::for_microphone(&mix_format),
        };
        let wave = format.to_wave_format();
        let (pointer, convert) = if format.is(&mix_format) {
            (mix.0.cast_const(), 0)
        } else {
            (
                (&raw const wave).cast::<WAVEFORMATEX>(),
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
            )
        };
        let loopback = match source {
            Source::Loopback => AUDCLNT_STREAMFLAGS_LOOPBACK,
            Source::Microphone => 0,
        };
        // Event-driven first; a fresh client without the event when that is refused. The event is
        // made before the client, so that on every early return below the client — a local made
        // after it — is released first (`WaitEvent`'s `Drop`).
        let event = WaitEvent::new()?;
        let evented = device.client()?;
        // SAFETY: a fresh client; `pointer` points at a format that outlives the call (`mix` or
        // `wave`, both locals of this frame).
        let started = unsafe {
            evented.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                loopback | convert | HIDDEN | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                CAPTURE_BUFFER_HNS,
                0,
                pointer,
                None,
            )
        }
        .and_then(|()| {
            // SAFETY: the client was initialised event-driven just now; the event outlives it on
            // every path (declared before it here, after it in `Self`).
            unsafe { evented.SetEventHandle(event.0) }
        });
        // `event` bound before `client`, so dropped after it on the early returns below.
        let (event, client) = match started {
            Ok(()) => (Some(event), evented),
            Err(error) => {
                log::debug!("capture without an event ({error}); polling instead");
                let polled = device.client()?;
                // SAFETY: as above, on another fresh client.
                unsafe {
                    polled.Initialize(
                        AUDCLNT_SHAREMODE_SHARED,
                        loopback | convert | HIDDEN,
                        CAPTURE_BUFFER_HNS,
                        0,
                        pointer,
                        None,
                    )
                }?;
                (None, polled)
            }
        };
        let mut period = 0_i64;
        // SAFETY: the client is initialised; `period` is a local.
        unsafe { client.GetDevicePeriod(Some(&raw mut period), None) }?;
        // SAFETY: the client is initialised.
        let capture = unsafe { client.GetService::<IAudioCaptureClient>() }?;
        Ok(Self {
            capture,
            client,
            event,
            format,
            period: format.frames_in(period),
            native_rate: mix_format.rate,
        })
    }

    pub(crate) const fn format(&self) -> StreamFormat {
        self.format
    }

    /// The rate the endpoint itself runs at, when it says: what a microphone really carries,
    /// recorded at 48 kHz whatever that is — the de-esser's source rate
    /// (`crate::lane_dsp::InputDsp::set_source_rate`).
    pub(crate) const fn native_rate(&self) -> Option<u32> {
        if self.native_rate == 0 {
            None
        } else {
            Some(self.native_rate)
        }
    }

    /// The frames one packet carries, about: the endpoint's period.
    pub(crate) const fn period(&self) -> usize {
        self.period
    }

    pub(crate) const fn event(&self) -> Option<&WaitEvent> {
        self.event.as_ref()
    }

    #[allow(unsafe_code)]
    pub(crate) fn start(&self) -> windows_core::Result<()> {
        // SAFETY: the client is initialised and alive.
        unsafe { self.client.Start() }
    }

    #[allow(unsafe_code)]
    pub(crate) fn stop(&self) {
        // SAFETY: as above. A stream already stopped answers `S_FALSE`.
        let _ = unsafe { self.client.Stop() };
    }

    /// Hand every packet waiting to `take`, oldest first, releasing each once it has been taken.
    ///
    /// Real-time safe: two calls into WASAPI per packet and the closure, no allocation.
    ///
    /// # Errors
    /// The `HRESULT` of a stream that has failed — `AUDCLNT_E_DEVICE_INVALIDATED` above all.
    #[allow(unsafe_code)]
    pub(crate) fn drain(&self, mut take: impl FnMut(Packet<'_>)) -> windows_core::Result<()> {
        let block_align = self.format.block_align();
        loop {
            // SAFETY: the capture client is alive.
            if unsafe { self.capture.GetNextPacketSize() }? == 0 {
                return Ok(());
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames = 0_u32;
            let mut flags = 0_u32;
            // SAFETY: the out-pointers are locals; the buffer returned is valid until the
            // `ReleaseBuffer` below and is read only before it.
            unsafe {
                self.capture
                    .GetBuffer(&raw mut data, &raw mut frames, &raw mut flags, None, None)
            }?;
            if frames == 0 {
                // `AUDCLNT_S_BUFFER_EMPTY`: nothing to release.
                return Ok(());
            }
            let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
            if silent || data.is_null() {
                take(Packet::Silence(frames as usize));
            } else {
                // SAFETY: WASAPI hands `frames` whole frames of the stream's format at `data`,
                // valid until the release below; the slice does not outlive this iteration.
                let bytes =
                    unsafe { std::slice::from_raw_parts(data, frames as usize * block_align) };
                take(Packet::Samples(bytes));
            }
            // SAFETY: releases exactly the packet taken above.
            unsafe { self.capture.ReleaseBuffer(frames) }?;
        }
    }
}

/// A shared-mode, event-driven render stream.
pub(crate) struct RenderStream {
    render: IAudioRenderClient,
    client: IAudioClient,
    event: WaitEvent,
    buffer: usize,
    period: usize,
    block_align: usize,
}

impl RenderStream {
    /// Open a render stream on `device` at `format` — the lane's own, whatever the device mixes
    /// at: `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM` with the default-quality resampler has the engine
    /// convert the rate and the channels, so the ring and the chain never do. That is what lets
    /// the output lane run at the virtual device's rate whatever the speakers run at, and replaces
    /// the original's integer up-sampler (`sndDevicesDoPlayback.cpp:79-89`, never ported).
    ///
    /// # Errors
    /// The `HRESULT` the endpoint refused the stream with: `AUDCLNT_E_DEVICE_IN_USE` for a device
    /// another application holds in exclusive mode, above all.
    #[allow(unsafe_code)]
    pub(crate) fn open(device: &Device, format: &StreamFormat) -> windows_core::Result<Self> {
        // The event first, the client after it: on every early return below the client, the
        // later local, is released before the event it was handed (`WaitEvent`'s `Drop`).
        let event = WaitEvent::new()?;
        let client = device.client()?;
        let wave = format.to_wave_format();
        // SAFETY: a fresh client; `wave` is a local that outlives the call.
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                    | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY
                    | HIDDEN,
                RENDER_BUFFER_HNS,
                0,
                (&raw const wave).cast::<WAVEFORMATEX>(),
                None,
            )
        }?;
        // SAFETY: the client was initialised event-driven; the event outlives it on every path
        // (declared before it here, after it in `Self`).
        unsafe { client.SetEventHandle(event.0) }?;
        // SAFETY: the client is initialised.
        let buffer = unsafe { client.GetBufferSize() }? as usize;
        let mut period = 0_i64;
        // SAFETY: as above; `period` is a local.
        unsafe { client.GetDevicePeriod(Some(&raw mut period), None) }?;
        // SAFETY: as above.
        let render = unsafe { client.GetService::<IAudioRenderClient>() }?;
        Ok(Self {
            render,
            client,
            event,
            buffer,
            period: format.frames_in(period).max(1),
            block_align: format.block_align(),
        })
    }

    pub(crate) const fn event(&self) -> &WaitEvent {
        &self.event
    }

    /// The buffer's size, in frames.
    pub(crate) const fn buffer(&self) -> usize {
        self.buffer
    }

    /// The frames the engine takes each time it wakes the stream.
    pub(crate) const fn period(&self) -> usize {
        self.period
    }

    /// Frames written and not yet played.
    ///
    /// # Errors
    /// The `HRESULT` of a stream that has failed.
    #[allow(unsafe_code)]
    pub(crate) fn padding(&self) -> windows_core::Result<usize> {
        // SAFETY: the client is alive.
        unsafe { self.client.GetCurrentPadding() }.map(|frames| frames as usize)
    }

    /// Write `frames` frames, filled by `fill` as little-endian `f32` bytes.
    ///
    /// Real-time safe: two calls into WASAPI and the closure.
    ///
    /// # Errors
    /// The `HRESULT` of a stream that has failed, or of a write past the free space.
    #[allow(unsafe_code)]
    pub(crate) fn write(
        &self,
        frames: usize,
        fill: impl FnOnce(&mut [u8]),
    ) -> windows_core::Result<()> {
        let Ok(count) = u32::try_from(frames) else {
            return Err(windows::Win32::Foundation::E_INVALIDARG.into());
        };
        if count == 0 {
            return Ok(());
        }
        // SAFETY: the render client is alive; the buffer returned holds `frames` frames and is
        // valid until the release below, and the slice made of it does not outlive this call.
        unsafe {
            let data = self.render.GetBuffer(count)?;
            if data.is_null() {
                return self.render.ReleaseBuffer(0, 0);
            }
            fill(std::slice::from_raw_parts_mut(
                data,
                frames * self.block_align,
            ));
            self.render.ReleaseBuffer(count, 0)
        }
    }

    #[allow(unsafe_code)]
    pub(crate) fn start(&self) -> windows_core::Result<()> {
        // SAFETY: the client is initialised and alive.
        unsafe { self.client.Start() }
    }

    #[allow(unsafe_code)]
    pub(crate) fn stop(&self) {
        // SAFETY: as above; a stream already stopped answers `S_FALSE`.
        let _ = unsafe { self.client.Stop() };
    }
}

// ---------------------------------------------------------------------------------------------
// The default endpoints
// ---------------------------------------------------------------------------------------------

/// `IPolicyConfig` itself (module docs), declared in a module of its own: the macro defines the
/// interface's vtable, its wrapper and the trait an implementation would implement, and only two
/// of its methods are ever called.
#[allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    clippy::upper_case_acronyms
)]
mod policy {
    use std::ffi::c_void;

    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::{ERole, WAVEFORMATEX};
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows_core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, PCWSTR};

    /// `CLSID_PolicyConfigClient` (EarTrumpet's `PolicyConfigClient`).
    pub(super) const CLSID_POLICY_CONFIG_CLIENT: GUID =
        GUID::from_u128(0x870a_f99c_171d_4f9e_af0d_e63d_f40c_2bc9);

    #[windows_core::interface("f8679f50-850a-41cf-9c72-430f290290c8")]
    pub(super) unsafe trait IPolicyConfig: IUnknown {
        pub(super) fn GetMixFormat(
            &self,
            device: PCWSTR,
            format: *mut *mut WAVEFORMATEX,
        ) -> HRESULT;
        fn GetDeviceFormat(
            &self,
            device: PCWSTR,
            default: i32,
            format: *mut *mut WAVEFORMATEX,
        ) -> HRESULT;
        fn ResetDeviceFormat(&self, device: PCWSTR) -> HRESULT;
        fn SetDeviceFormat(
            &self,
            device: PCWSTR,
            endpoint: *mut WAVEFORMATEX,
            mix: *mut WAVEFORMATEX,
        ) -> HRESULT;
        fn GetProcessingPeriod(
            &self,
            device: PCWSTR,
            default: i32,
            default_period: *mut i64,
            minimum_period: *mut i64,
        ) -> HRESULT;
        fn SetProcessingPeriod(&self, device: PCWSTR, period: *mut i64) -> HRESULT;
        fn GetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
        fn SetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
        fn GetPropertyValue(
            &self,
            device: PCWSTR,
            key: *const PROPERTYKEY,
            value: *mut PROPVARIANT,
        ) -> HRESULT;
        fn SetPropertyValue(
            &self,
            device: PCWSTR,
            key: *const PROPERTYKEY,
            value: *mut PROPVARIANT,
        ) -> HRESULT;
        pub(super) fn SetDefaultEndpoint(&self, device: PCWSTR, role: ERole) -> HRESULT;
        fn SetEndpointVisibility(&self, device: PCWSTR, visible: i32) -> HRESULT;
    }

    // The vtable the macro lays out is the interface's: three `IUnknown` slots and twelve more.
    const _: () = assert!(
        size_of::<IPolicyConfig_Vtbl>() == size_of::<IUnknown_Vtbl>() + 12 * size_of::<usize>()
    );
}

/// The system's default-device policy (module docs, "IPolicyConfig").
pub(crate) struct PolicyConfig(policy::IPolicyConfig);

impl PolicyConfig {
    /// # Errors
    /// When the class is not there, or does not have the interface — a Windows that changed it.
    #[allow(unsafe_code)]
    pub(crate) fn new() -> windows_core::Result<Self> {
        // SAFETY: no aggregation; the calling thread is in the MTA. `CoCreateInstance` asks the
        // object for exactly this IID, so the pointer it returns has this vtable or the call
        // fails with `E_NOINTERFACE`.
        unsafe {
            CoCreateInstance::<_, policy::IPolicyConfig>(
                &policy::CLSID_POLICY_CONFIG_CLIENT,
                None,
                CLSCTX_ALL,
            )
        }
        .map(Self)
    }

    /// Make the endpoint `id` the default of its direction for `role`.
    ///
    /// # Errors
    /// The `HRESULT` the policy refused with.
    #[allow(unsafe_code)]
    pub(crate) fn set_default(&self, id: &str, role: Role) -> windows_core::Result<()> {
        let id = wide(id);
        // SAFETY: the object is alive; `id` is NUL-terminated and outlives the call; `ERole` is
        // the `i32` the slot takes.
        unsafe {
            self.0
                .SetDefaultEndpoint(PCWSTR(id.as_ptr()), self::role(role))
        }
        .ok()
    }

    /// The mix format of the endpoint `id`, read through the policy: what the live check holds
    /// to `IAudioClient::GetMixFormat` to prove the class and the vtable right without changing
    /// anything.
    ///
    /// # Errors
    /// The `HRESULT` the policy answered with.
    #[allow(unsafe_code)]
    pub(crate) fn mix_format(&self, id: &str) -> windows_core::Result<MixFormat> {
        let id = wide(id);
        let mut format: *mut WAVEFORMATEX = std::ptr::null_mut();
        // SAFETY: the object is alive; `id` outlives the call and `format` is a local the callee
        // fills with a `CoTaskMemAlloc`ed format, freed by `MixFormatPtr`.
        unsafe { self.0.GetMixFormat(PCWSTR(id.as_ptr()), &raw mut format) }.ok()?;
        if format.is_null() {
            return Err(windows::Win32::Foundation::E_POINTER.into());
        }
        Ok(MixFormatPtr(format).read())
    }
}

// ---------------------------------------------------------------------------------------------
// Processes
// ---------------------------------------------------------------------------------------------

/// A handle closed when dropped.
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: a handle this module opened and owns, closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// This process's ID.
#[allow(unsafe_code)]
pub(crate) fn own_pid() -> u32 {
    // SAFETY: no arguments, no failure.
    unsafe { GetCurrentProcessId() }
}

/// The full path of the image process `pid` runs, as `QueryFullProcessImageNameW` gives it; `None`
/// when it cannot be read — a protected process, one that has exited.
#[allow(unsafe_code)]
pub(crate) fn process_image(pid: u32) -> Option<String> {
    // SAFETY: no handle is inherited; a failure is an error, not a handle.
    let process =
        OwnedHandle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?);
    let mut path = [0_u16; 1024];
    let mut length = path.len() as u32;
    // SAFETY: `path` is a local buffer of `length` characters, which the call fills and updates.
    unsafe {
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(path.as_mut_ptr()),
            &raw mut length,
        )
    }
    .ok()?;
    path.get(..length as usize).map(String::from_utf16_lossy)
}

/// A process watched for its end: a handle that can be waited on, which also keeps the process's
/// ID from being given to another while it is held. What the original FxSound is watched with once
/// it has been found, so that its exit is seen without taking the process list again.
pub(crate) struct ProcessWatch(OwnedHandle);

impl ProcessWatch {
    /// Watch process `pid`; `None` when it cannot be opened for that — it has exited, or is
    /// another user's.
    #[allow(unsafe_code)]
    pub(crate) fn open(pid: u32) -> Option<Self> {
        // SAFETY: no handle is inherited; a failure is an error, not a handle.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }.ok()?;
        Some(Self(OwnedHandle(handle)))
    }

    /// Whether the process has ended — or can no longer be waited on, which is taken for the same.
    #[allow(unsafe_code)]
    pub(crate) fn has_exited(&self) -> bool {
        // SAFETY: the handle is alive (owned by `self`); a wait of no time.
        (unsafe { WaitForSingleObject(self.0.0, 0) }) != WAIT_TIMEOUT
    }
}

/// Every running process, by ID and executable name, with the image path of those that are
/// called `FxSound.exe` ([`original::IMAGE_NAME`]): what [`original::find`] looks among. Empty
/// when the snapshot cannot be taken.
#[allow(unsafe_code)]
pub(crate) fn processes() -> Vec<Process> {
    // SAFETY: a snapshot of every process; a failure is an error, not a handle.
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return Vec::new();
    };
    let snapshot = OwnedHandle(snapshot);
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut found = Vec::new();
    // SAFETY: `entry` is a local whose `dwSize` says how large it is, as the call requires.
    let mut more = unsafe { Process32FirstW(snapshot.0, &raw mut entry) }.is_ok();
    while more {
        let name = &entry.szExeFile;
        let length = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        let exe = String::from_utf16_lossy(&name[..length]);
        let pid = entry.th32ProcessID;
        let image = (exe.to_lowercase() == original::IMAGE_NAME)
            .then(|| process_image(pid))
            .flatten();
        found.push(Process { pid, exe, image });
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot.0, &raw mut entry) }.is_ok();
    }
    found
}

/// Whether `error` is a stream's device going away under it (`AUDCLNT_E_DEVICE_INVALIDATED`,
/// `AUDCLNT_E_RESOURCES_INVALIDATED`) or not being there ([`E_NOTFOUND`]): a reason to read the
/// endpoints again and choose afresh, not a failure to report.
pub(crate) fn is_device_gone(error: &windows_core::Error) -> bool {
    use windows::Win32::Media::Audio::{
        AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_E_RESOURCES_INVALIDATED,
    };
    let code = error.code();
    code == AUDCLNT_E_DEVICE_INVALIDATED
        || code == AUDCLNT_E_RESOURCES_INVALIDATED
        || code == E_NOTFOUND
}

/// Whether `error` is the audio service having gone (`AUDCLNT_E_SERVICE_NOT_RUNNING`, or the RPC
/// to it broken): the Windows face of PipeWire's socket closing (`docs/spec/12-audio-io.md` §22).
pub(crate) fn is_service_gone(error: &windows_core::Error) -> bool {
    use windows::Win32::Media::Audio::AUDCLNT_E_SERVICE_NOT_RUNNING;
    /// `RPC_E_DISCONNECTED`: the object's server went away.
    const RPC_E_DISCONNECTED: HRESULT = HRESULT(0x8001_0108_u32 as i32);
    /// `HRESULT_FROM_WIN32(RPC_S_SERVER_UNAVAILABLE)`.
    const RPC_S_SERVER_UNAVAILABLE: HRESULT = HRESULT(0x8007_06BA_u32 as i32);
    let code = error.code();
    code == AUDCLNT_E_SERVICE_NOT_RUNNING
        || code == RPC_E_DISCONNECTED
        || code == RPC_S_SERVER_UNAVAILABLE
}

/// Whether `error` is the format being refused.
pub(crate) fn is_format_refused(error: &windows_core::Error) -> bool {
    use windows::Win32::Media::Audio::AUDCLNT_E_UNSUPPORTED_FORMAT;
    error.code() == AUDCLNT_E_UNSUPPORTED_FORMAT
}

/// Whether `error` is the device being held by another application in exclusive mode.
pub(crate) fn is_device_in_use(error: &windows_core::Error) -> bool {
    use windows::Win32::Media::Audio::AUDCLNT_E_DEVICE_IN_USE;
    error.code() == AUDCLNT_E_DEVICE_IN_USE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_volume_is_its_level_as_an_amplitude_and_nothing_when_muted_or_at_the_bottom() {
        assert_eq!(gain_of(false, 1.0, 0.0), 1.0);
        assert!((gain_of(false, 0.5, -6.0) - 0.501).abs() < 1e-3);
        assert_eq!(gain_of(true, 1.0, 0.0), 0.0, "muted");
        assert_eq!(gain_of(false, 0.0, -96.0), 0.0, "the slider at the bottom");
        assert_eq!(gain_of(false, f32::NAN, 0.0), 0.0);
        assert_eq!(gain_of(false, 1.0, f32::NEG_INFINITY), 0.0);
        assert_eq!(gain_of(false, 1.0, 6.0), 1.0, "never above unity");
    }

    #[test]
    fn a_wide_string_ends_in_its_nul() {
        assert_eq!(wide("ab"), [97, 98, 0]);
        assert_eq!(wide(""), [0]);
        assert_eq!(*wide("Ж").first().expect("a character"), 0x0416);
    }
}
