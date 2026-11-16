//! Suspend and resume on Windows: what logind's `PrepareForSleep` is to the Linux build
//! (`crate::sleep` there).
//!
//! `PowerRegisterSuspendResumeNotification` calls back as the machine goes to sleep
//! (`PBT_APMSUSPEND`) and once it is back (`PBT_APMRESUMEAUTOMATIC`, and `PBT_APMRESUMESUSPEND`
//! too when the user is there to have woken it). Each reaches the GUI thread as the same word the
//! Linux watcher sends — `true` going down, `false` back up — and the controller does the rest
//! (`App::system_sleeping`): both lanes muted on the way down, started clean on the way up. A
//! resume heard twice is said to the engine once.
//!
//! Nothing holds the suspend up, as on Linux: the callback hands the word over and returns.
//! The original does the same from this very notification (d62e024, `FxController.cpp:2145-2159`).

use std::ffi::c_void;

use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE};
use windows::Win32::System::Power::{
    DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, HPOWERNOTIFY, PowerRegisterSuspendResumeNotification,
    PowerUnregisterSuspendResumeNotification,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND,
};

use crate::wake::WakingSender;

/// What a power broadcast says about sleep: `Some(true)` going to sleep, `Some(false)` back from
/// it, `None` for the others (a battery's state, a power scheme changing).
#[must_use]
pub const fn sleeping(broadcast: u32) -> Option<bool> {
    match broadcast {
        PBT_APMSUSPEND => Some(true),
        PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => Some(false),
        _ => None,
    }
}

/// The callback Windows makes, on a thread of its own.
unsafe extern "system" fn on_power(
    context: *const c_void,
    broadcast: u32,
    _setting: *const c_void,
) -> u32 {
    // SAFETY: `context` is the sender `SleepWatch::start` registered, which lives as long as the
    // process (see `SleepWatch`'s docs).
    let sender = unsafe { &*context.cast::<WakingSender<bool>>() };
    if let Some(word) = sleeping(broadcast) {
        // A full or closed channel is a GUI thread on its way out; nothing to hold up for.
        let _ = sender.try_send(word);
    }
    ERROR_SUCCESS.0
}

/// The GUI thread's end of the registration. Dropping it unregisters.
///
/// The sender the callback holds is never freed: Windows promises nothing about a callback still
/// running as the registration goes, and one sender, once per process, costs less than that
/// question.
#[derive(Debug)]
pub struct SleepWatch {
    registration: Option<isize>,
}

impl SleepWatch {
    /// Listen for suspend and resume, and send what each says on `sleeping`. Never fails: without
    /// the registration — it is refused only to a service — CrossFX runs on without it, as the
    /// Linux build does without a system bus.
    #[must_use]
    pub fn start(sleeping: impl Into<WakingSender<bool>>) -> Self {
        let context: &'static WakingSender<bool> = Box::leak(Box::new(sleeping.into()));
        // Read by Windows during the call only; the context outlives everything.
        let parameters = DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(on_power),
            Context: std::ptr::from_ref(context).cast_mut().cast(),
        };
        let mut registration: *mut c_void = std::ptr::null_mut();
        // SAFETY: `parameters` and `registration` outlive the call; the context lives for ever.
        let status = unsafe {
            PowerRegisterSuspendResumeNotification(
                DEVICE_NOTIFY_CALLBACK,
                HANDLE(std::ptr::from_ref(&parameters).cast_mut().cast()),
                &raw mut registration,
            )
        };
        if status != ERROR_SUCCESS {
            log::warn!("not watching for suspend: {}", super::win32_error(status));
            return Self { registration: None };
        }
        log::debug!("watching for suspend and resume");
        Self {
            registration: Some(registration as isize),
        }
    }

    /// Whether the registration was made.
    #[must_use]
    pub const fn is_listening(&self) -> bool {
        self.registration.is_some()
    }
}

impl Drop for SleepWatch {
    fn drop(&mut self) {
        if let Some(registration) = self.registration.take() {
            // SAFETY: the handle the registration gave back, unregistered once.
            let status =
                unsafe { PowerUnregisterSuspendResumeNotification(HPOWERNOTIFY(registration)) };
            if status != ERROR_SUCCESS {
                log::debug!(
                    "the suspend watcher did not unregister: {}",
                    super::win32_error(status)
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;

    #[test]
    fn suspend_is_sleeping_and_either_resume_is_awake() {
        assert_eq!(sleeping(PBT_APMSUSPEND), Some(true));
        assert_eq!(sleeping(PBT_APMRESUMEAUTOMATIC), Some(false));
        assert_eq!(sleeping(PBT_APMRESUMESUSPEND), Some(false));
        // `PBT_APMPOWERSTATUSCHANGE`, `PBT_POWERSETTINGCHANGE`: not about sleep.
        assert_eq!(sleeping(10), None);
        assert_eq!(sleeping(0x8013), None);
    }

    #[test]
    fn the_callback_hands_the_word_to_the_gui_thread() {
        let (tx, rx) = unbounded();
        let sender = WakingSender::from(tx);
        for (broadcast, word) in [(PBT_APMSUSPEND, true), (PBT_APMRESUMEAUTOMATIC, false)] {
            // SAFETY: the context is a live sender, as the registration's is.
            let answer = unsafe {
                on_power(
                    std::ptr::from_ref(&sender).cast(),
                    broadcast,
                    std::ptr::null(),
                )
            };
            assert_eq!(answer, ERROR_SUCCESS.0);
            assert_eq!(rx.try_recv(), Ok(word));
        }
        // SAFETY: as above.
        unsafe { on_power(std::ptr::from_ref(&sender).cast(), 10, std::ptr::null()) };
        assert!(rx.try_recv().is_err(), "a battery's news is not sleep");
    }

    #[test]
    fn this_session_takes_the_registration_and_gives_it_back() {
        let (tx, _rx) = unbounded();
        let watch = SleepWatch::start(tx);
        assert!(watch.is_listening());
        drop(watch);
    }
}
