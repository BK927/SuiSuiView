//! Only query the OS-visible STANDARD default. Do not enumerate installed ICC
//! files: that would bypass Advanced Color's sRGB/synthetic-profile contract.
#![allow(unsafe_code)]
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_FILE_NOT_FOUND, ERROR_NOT_FOUND, ERROR_PROFILE_NOT_ASSOCIATED_WITH_DEVICE,
};
use windows_sys::Win32::UI::ColorSystem::{
    GetColorDirectoryW, WcsGetDefaultColorProfile, WcsGetDefaultColorProfileSize,
    WcsGetUsePerUserProfiles, CPST_STANDARD_DISPLAY_COLOR_MODE, CPT_ICC,
    WCS_PROFILE_MANAGEMENT_SCOPE_CURRENT_USER, WCS_PROFILE_MANAGEMENT_SCOPE_SYSTEM_WIDE,
};

const CLASS_MONITOR: u32 = u32::from_be_bytes(*b"mntr");
const MAX_PROFILE_NAME_BYTES: u32 = 64 * 1024;

pub(super) fn default_profile(display: &str) -> Result<Option<PathBuf>, String> {
    let display: Vec<u16> = display.encode_utf16().chain(Some(0)).collect();
    let mut per_user = 0;
    // SAFETY: display is NUL-terminated and all output pointers are valid.
    if unsafe { WcsGetUsePerUserProfiles(display.as_ptr(), CLASS_MONITOR, &mut per_user) } == 0 {
        return Err(last_error("Cannot query display profile policy"));
    }
    let scope = if per_user != 0 {
        WCS_PROFILE_MANAGEMENT_SCOPE_CURRENT_USER
    } else {
        WCS_PROFILE_MANAGEMENT_SCOPE_SYSTEM_WIDE
    };
    let mut bytes = 0;
    // SAFETY: display and bytes outlive this call; no pointers are retained.
    let found = unsafe {
        WcsGetDefaultColorProfileSize(
            scope,
            display.as_ptr(),
            CPT_ICC,
            CPST_STANDARD_DISPLAY_COLOR_MODE,
            0,
            &mut bytes,
        )
    };
    if found == 0 {
        // "No profile" is deliberately sRGB under Windows Advanced Color.
        let error = unsafe { GetLastError() };
        if matches!(
            error,
            ERROR_FILE_NOT_FOUND | ERROR_NOT_FOUND | ERROR_PROFILE_NOT_ASSOCIATED_WITH_DEVICE
        ) {
            return Ok(None);
        }
        return Err(format!("Cannot query display profile ({error})"));
    }
    if bytes == 0 {
        return Ok(None);
    }
    if bytes > MAX_PROFILE_NAME_BYTES || bytes % 2 != 0 {
        return Err("Invalid display profile name size".to_owned());
    }
    let mut name = vec![0; bytes as usize / 2];
    // SAFETY: name has exactly the byte capacity requested by the OS.
    if unsafe {
        WcsGetDefaultColorProfile(
            scope,
            display.as_ptr(),
            CPT_ICC,
            CPST_STANDARD_DISPLAY_COLOR_MODE,
            0,
            bytes,
            name.as_mut_ptr(),
        )
    } == 0
    {
        return Err(last_error("Cannot read display profile name"));
    }
    let path = PathBuf::from(wide_string(&name));
    if path.as_os_str().is_empty() {
        return Ok(None);
    }
    if path.is_absolute() {
        return Ok(Some(path));
    }
    let mut directory_bytes = 0;
    // SAFETY: the initial null buffer is the documented size-query pattern.
    unsafe { GetColorDirectoryW(std::ptr::null(), std::ptr::null_mut(), &mut directory_bytes) };
    if directory_bytes == 0 || directory_bytes > MAX_PROFILE_NAME_BYTES || directory_bytes % 2 != 0
    {
        return Err(last_error("Cannot query color directory"));
    }
    let mut directory = vec![0; directory_bytes as usize / 2];
    // SAFETY: directory provides the OS-requested writable byte capacity.
    if unsafe {
        GetColorDirectoryW(
            std::ptr::null(),
            directory.as_mut_ptr(),
            &mut directory_bytes,
        )
    } == 0
    {
        return Err(last_error("Cannot read color directory"));
    }
    Ok(Some(PathBuf::from(wide_string(&directory)).join(path)))
}

fn wide_string(value: &[u16]) -> OsString {
    OsString::from_wide(&value[..value.iter().position(|v| *v == 0).unwrap_or(value.len())])
}

fn last_error(context: &str) -> String {
    // SAFETY: GetLastError has no preconditions and does not retain resources.
    format!("{context} ({})", unsafe { GetLastError() })
}
