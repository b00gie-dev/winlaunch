//! Minimal HTTPS GET over WinHTTP (part of Windows): no extra crate, and winhttp.dll
//! is delay-loaded, so it costs nothing until the first request. Uses the system
//! proxy settings. Call only from background threads (it blocks).

use crate::wide;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Networking::WinHttp::*;

/// Largest response we accept (the ECB rates file is about 1.5 KB).
const MAX_BYTES: usize = 1 << 20;

/// Closes a WinHTTP handle when dropped.
struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

/// Body of `https://{host}{path}` if the server answers 200 OK.
pub fn get(host: &str, path: &str) -> Result<Vec<u8>, String> {
    request(true, host, INTERNET_DEFAULT_HTTPS_PORT, path)
}

/// Parts of an http(s) URL: (https, host, port, path with query). `None` for other
/// schemes or URLs without a host.
pub fn split_url(url: &str) -> Option<(bool, String, u16, String)> {
    let (secure, rest) = match url.strip_prefix("https://") {
        Some(r) => (true, r),
        None => (false, url.strip_prefix("http://")?),
    };
    let (authority, path) = match rest.find(['/', '?', '#']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let path = path.split('#').next().unwrap_or("/");
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let authority = authority.rsplit('@').next()?; // drop user:password@
    let default_port = if secure { 443 } else { 80 };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            (h, p.parse().ok()?)
        }
        _ => (authority, default_port),
    };
    (!host.is_empty()).then(|| (secure, host.to_string(), port, path))
}

/// Body of an http or https URL if the server answers 200 OK.
pub fn get_url(url: &str) -> Result<Vec<u8>, String> {
    let (secure, host, port, path) = split_url(url).ok_or("not an http(s) URL")?;
    request(secure, &host, port, &path)
}

fn request(secure: bool, host: &str, port: u16, path: &str) -> Result<Vec<u8>, String> {
    let fail = |what: &str| format!("{what} failed: error {}", std::io::Error::last_os_error());
    unsafe {
        let session = Handle(WinHttpOpen(
            wide(concat!("WinLaunch/", env!("CARGO_PKG_VERSION"))).as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            null(),
            null(),
            0,
        ));
        if session.0.is_null() {
            return Err(fail("WinHttpOpen"));
        }
        // Resolve, connect, send, receive: 10 s each at most.
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 10_000, 10_000);
        let connect = Handle(WinHttpConnect(session.0, wide(host).as_ptr(), port, 0));
        if connect.0.is_null() {
            return Err(fail("WinHttpConnect"));
        }
        let request = Handle(WinHttpOpenRequest(
            connect.0,
            wide("GET").as_ptr(),
            wide(path).as_ptr(),
            null(),
            null(),
            null(),
            if secure { WINHTTP_FLAG_SECURE } else { 0 },
        ));
        if request.0.is_null() {
            return Err(fail("WinHttpOpenRequest"));
        }
        if WinHttpSendRequest(request.0, null(), 0, null(), 0, 0, 0) == 0 {
            return Err(fail("WinHttpSendRequest"));
        }
        if WinHttpReceiveResponse(request.0, null_mut()) == 0 {
            return Err(fail("WinHttpReceiveResponse"));
        }
        let mut status = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            &mut status as *mut u32 as _,
            &mut size,
            null_mut(),
        );
        if status != 200 {
            return Err(format!("HTTP status {status}"));
        }
        let mut body = Vec::new();
        loop {
            let mut available = 0u32;
            if WinHttpQueryDataAvailable(request.0, &mut available) == 0 {
                return Err(fail("WinHttpQueryDataAvailable"));
            }
            if available == 0 {
                break;
            }
            let start = body.len();
            body.resize(start + available as usize, 0);
            let mut read = 0u32;
            if WinHttpReadData(
                request.0,
                body[start..].as_mut_ptr() as _,
                available,
                &mut read,
            ) == 0
            {
                return Err(fail("WinHttpReadData"));
            }
            body.truncate(start + read as usize);
            if body.len() > MAX_BYTES {
                return Err("response too large".into());
            }
        }
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_urls() {
        let parts = |secure, host: &str, port, path: &str| {
            Some((secure, host.to_string(), port, path.to_string()))
        };
        assert_eq!(
            split_url("https://www.google.com/search?q={query}"),
            parts(true, "www.google.com", 443, "/search?q={query}")
        );
        assert_eq!(
            split_url("http://homeassistant.local:8123"),
            parts(false, "homeassistant.local", 8123, "/")
        );
        assert_eq!(
            split_url("https://x.org?a=1#top"),
            parts(true, "x.org", 443, "/?a=1")
        );
        assert_eq!(
            split_url("https://user:pw@x.org/a"),
            parts(true, "x.org", 443, "/a")
        );
        assert_eq!(split_url("ftp://x.org"), None);
        assert_eq!(split_url("https://"), None);
    }
}
