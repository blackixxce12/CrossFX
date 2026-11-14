//! Who this process runs as, for the control pipe's name and its access control.
//!
//! A named pipe lives in one namespace for the whole machine, so its name alone protects nothing:
//! any session's process can create `\\.\pipe\fxsound-…` first, and connect to it. Three things
//! keep the control pipe this user's own ([`super::pipe`]): its security descriptor grants access
//! to the user's SID and to nobody else, at medium integrity and above; a client checks that the
//! pipe it reached is owned by that SID before it hands over a command line, so that a pipe
//! another account put there first learns nothing; and the client checks the pipe's integrity
//! label too ([`integrity_of`]), since an owner is only a name its creator gave: a process of this
//! very user at low integrity — a sandboxed renderer or viewer — may name the user as the owner of
//! a pipe it makes, but cannot label the pipe above its own level.

use std::io;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};

use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows::Win32::Security::{
    ACE_HEADER, ACL, GetAce, GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation,
    LABEL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SYSTEM_MANDATORY_LABEL_ACE, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{PCWSTR, PWSTR};

use super::{io_error, wide, win32_error};

/// The medium integrity level, `SECURITY_MANDATORY_MEDIUM_RID`: a user's own programs run at it,
/// and an object with no label of its own is read as carrying it.
pub(crate) const MEDIUM_INTEGRITY: u32 = 0x2000;

/// `SYSTEM_MANDATORY_LABEL_ACE_TYPE`: the ACE in a SACL that carries an object's integrity label.
const MANDATORY_LABEL_ACE: u8 = 0x11;

/// The SID of the account this process runs as, in its string form (`S-1-5-21-…`).
///
/// # Errors
///
/// If the process token cannot be read, which leaves FxSound without a pipe name it could trust.
pub(crate) fn sid() -> io::Result<String> {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo-handle of this process, and an out-pointer to a local.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) }
        .map_err(|e| io_error(&e))?;
    // SAFETY: `token` is a fresh handle we own from here on.
    let token = unsafe { OwnedHandle::from_raw_handle(token.0) };
    let token = HANDLE(token.as_raw_handle());

    let mut needed = 0_u32;
    // SAFETY: a size query: no buffer, and an out-pointer to a local. It fails with
    // `ERROR_INSUFFICIENT_BUFFER`, which is the answer.
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &raw mut needed) };
    // `TOKEN_USER` holds pointers into the same buffer, so it is allocated as `u64`s for their
    // alignment rather than as bytes.
    let words = (needed as usize).div_ceil(size_of::<u64>()).max(1);
    let mut buffer = vec![0_u64; words];
    // SAFETY: `buffer` is at least `needed` bytes long and outlives the call.
    unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            needed,
            &raw mut needed,
        )
    }
    .map_err(|e| io_error(&e))?;
    // SAFETY: the call above filled the buffer with a `TOKEN_USER` and its SID, suitably aligned.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    sid_string(user.User.Sid)
}

/// `sid` as `S-1-…`.
fn sid_string(sid: PSID) -> io::Result<String> {
    let mut text = PWSTR::null();
    // SAFETY: `sid` points at a valid SID for the length of the call; the string is ours to free.
    unsafe { ConvertSidToStringSidW(sid, &raw mut text) }.map_err(|e| io_error(&e))?;
    // SAFETY: a NUL-terminated string the call allocated with `LocalAlloc`.
    let string = unsafe { text.to_string() };
    // SAFETY: allocated by `ConvertSidToStringSidW`, freed once.
    unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
    string.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// The owner of the kernel object `handle` refers to, as a SID string: for the client's end of a
/// named pipe, whoever created the pipe — or whom its creator named.
pub(crate) fn owner_of(handle: HANDLE) -> io::Result<String> {
    let mut owner = PSID::default();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: out-pointers to locals; `owner` points into `descriptor`, which is ours to free.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(&raw mut owner),
            None,
            None,
            None,
            Some(&raw mut descriptor),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(win32_error(status));
    }
    let sid = sid_string(owner);
    // SAFETY: allocated by `GetSecurityInfo`, freed once, after the last use of `owner`.
    unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
    sid
}

/// The integrity level of the kernel object `handle` refers to: the RID of the mandatory label in
/// its security descriptor (`0x1000` low, [`MEDIUM_INTEGRITY`], `0x3000` high), or `None` for an
/// object that carries no label. Windows labels whatever a process below medium integrity makes
/// with that process's level, and a process may label what it makes at its own level or lower,
/// never higher: so an object labelled medium or above, or not at all, was made at medium or
/// above. For the client's end of a named pipe, the label is the one the pipe was made with.
pub(crate) fn integrity_of(handle: HANDLE) -> io::Result<Option<u32>> {
    let mut sacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: out-pointers to locals; `sacl` points into `descriptor`, which is ours to free.
    // Reading the label needs no more than the `READ_CONTROL` every handle opened for reading has.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            LABEL_SECURITY_INFORMATION,
            None,
            None,
            None,
            Some(&raw mut sacl),
            Some(&raw mut descriptor),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(win32_error(status));
    }
    // SAFETY: `sacl` is null or the SACL inside `descriptor`, which is still alive.
    let level = unsafe { mandatory_level(sacl) };
    // SAFETY: allocated by `GetSecurityInfo`, freed once, after the last use of `sacl`.
    unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
    Ok(level)
}

/// The RID of the mandatory label in `sacl`, if it has one.
///
/// # Safety
///
/// `sacl` is null or points at a valid ACL that outlives the call.
unsafe fn mandatory_level(sacl: *const ACL) -> Option<u32> {
    // SAFETY: as the caller promises.
    let acl = unsafe { sacl.as_ref() }?;
    for index in 0..u32::from(acl.AceCount) {
        let mut ace: *mut std::ffi::c_void = std::ptr::null_mut();
        // SAFETY: an index below the ACL's own count, and an out-pointer to a local; the ACE
        // returned points into the ACL.
        if unsafe { GetAce(sacl, index, &raw mut ace) }.is_err() || ace.is_null() {
            continue;
        }
        // SAFETY: every ACE starts with its header.
        if unsafe { (*ace.cast::<ACE_HEADER>()).AceType } != MANDATORY_LABEL_ACE {
            continue;
        }
        let label = ace.cast::<SYSTEM_MANDATORY_LABEL_ACE>();
        // SAFETY: a label ACE's SID starts at its `SidStart` and is part of the ACE; the
        // subauthorities read are within the count the SID gives.
        unsafe {
            let sid = PSID((&raw mut (*label).SidStart).cast());
            let count = *GetSidSubAuthorityCount(sid);
            return Some(match count {
                0 => 0,
                count => *GetSidSubAuthority(sid, u32::from(count) - 1),
            });
        }
    }
    None
}

/// A self-relative security descriptor, freed when dropped.
pub(crate) struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

// SAFETY: the descriptor is an immutable block of memory nothing else refers to; the pipe calls
// only read it.
unsafe impl Send for SecurityDescriptor {}
// SAFETY: as above.
unsafe impl Sync for SecurityDescriptor {}

impl std::fmt::Debug for SecurityDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecurityDescriptor").finish_non_exhaustive()
    }
}

impl SecurityDescriptor {
    /// Owned by `sid` and open to `sid` alone: `GENERIC_ALL` for that one account, a protected
    /// DACL so that nothing is inherited, and so no access at all for any other account, for
    /// anonymous callers or for an AppContainer. A process of the same user at low integrity cannot
    /// reach the pipe either, by the medium mandatory label (`S:(ML;;NW;;;ME)`): that is the
    /// server's half. The label is also what a client knows FxSound's pipe by
    /// ([`integrity_of`]), since a low-integrity process could make a pipe of the same name and
    /// owner, but not one labelled medium.
    ///
    /// The owner and the label are named rather than left to the token's defaults. An elevated
    /// administrator's default owner is the Administrators group, and a client compares the owner
    /// with its own SID; and an object an elevated process creates is labelled high, which no
    /// process at medium may write to. Named, the same user's FxSound and its command line meet
    /// on the pipe whichever of the two runs elevated — a start from an elevated terminal, a
    /// keybind from the desktop.
    pub(crate) fn only(sid: &str) -> io::Result<Self> {
        Self::parse(&format!("O:{sid}D:P(A;;GA;;;{sid})S:(ML;;NW;;;ME)"))
    }

    /// The descriptor `sddl` describes.
    pub(crate) fn parse(sddl: &str) -> io::Result<Self> {
        let sddl = wide(sddl);
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `sddl` is NUL-terminated and outlives the call; the descriptor is ours to free.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &raw mut descriptor,
                None,
            )
        }
        .map_err(|e| io_error(&e))?;
        Ok(Self(descriptor))
    }

    pub(crate) const fn as_ptr(&self) -> *mut std::ffi::c_void {
        self.0.0
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: allocated with `LocalAlloc` by the conversion, freed once.
        unsafe { LocalFree(Some(HLOCAL(self.0.0))) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_has_a_user_sid() {
        let sid = sid().expect("the token is readable");
        assert!(sid.starts_with("S-1-"), "{sid}");
        assert!(sid.split('-').count() >= 3, "{sid}");
    }

    #[test]
    fn a_descriptor_for_this_user_can_be_built() {
        let sid = sid().expect("the token is readable");
        let descriptor = SecurityDescriptor::only(&sid).expect("a valid SDDL");
        assert!(!descriptor.as_ptr().is_null());
        assert!(
            SecurityDescriptor::only("not a SID").is_err(),
            "an account that cannot be named is refused, not ignored"
        );
    }
}
