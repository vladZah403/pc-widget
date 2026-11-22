//! HTTPS downloads with WinHTTP and SHA-256 with CNG: what the update check
//! and the updater need, without a TLS library of our own. WinHTTP trusts
//! Windows' certificates and fetches a missing root itself (1.8's Python
//! couldn't: a fresh Windows 11 failed every check), and uses the system
//! proxy.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Networking::WinHttp::*;

use crate::wide;

/// A WinHTTP handle, closed when dropped.
struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

fn last_error(what: &str) -> String {
    let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    // 12002 timeout, 12007 name not resolved, 12029 cannot connect, 12175 certificate
    format!("{what} failed (WinHTTP {code})")
}

/// GET `url` (https only) and return the body, at most `max_bytes` of it.
/// `progress` hears the bytes read so far. Redirects (GitHub's downloads
/// go to its CDN) are followed, never from https to http.
pub fn get(
    url: &str,
    agent: &str,
    headers: &[(&str, &str)],
    max_bytes: u64,
    progress: &mut dyn FnMut(u64),
) -> Result<Vec<u8>, String> {
    let rest = url.strip_prefix("https://").ok_or("not an https address")?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if host.is_empty() {
        return Err("no host in the address".into());
    }
    unsafe {
        let agent = wide(agent);
        let mut session =
            Handle(WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, null(), null(), 0));
        if session.0.is_null() {
            // before Windows 8.1 there is no automatic proxy
            session =
                Handle(WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, null(), null(), 0));
        }
        if session.0.is_null() {
            return Err(last_error("WinHttpOpen"));
        }
        // resolve, connect, send, receive (ms)
        WinHttpSetTimeouts(session.0, 10_000, 15_000, 15_000, 30_000);
        let host_w = wide(host);
        let conn = Handle(WinHttpConnect(session.0, host_w.as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0));
        if conn.0.is_null() {
            return Err(last_error("connecting"));
        }
        let (verb, path_w) = (wide("GET"), wide(path));
        let req = Handle(WinHttpOpenRequest(
            conn.0,
            verb.as_ptr(),
            path_w.as_ptr(),
            null(),
            null(),
            null(),
            WINHTTP_FLAG_SECURE,
        ));
        if req.0.is_null() {
            return Err(last_error("the request"));
        }
        if !headers.is_empty() {
            let text: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
            let text = wide(&text);
            WinHttpAddRequestHeaders(req.0, text.as_ptr(), u32::MAX, WINHTTP_ADDREQ_FLAG_ADD);
        }
        if WinHttpSendRequest(req.0, null(), 0, null(), 0, 0, 0) == 0 {
            return Err(last_error("sending"));
        }
        if WinHttpReceiveResponse(req.0, null_mut()) == 0 {
            return Err(last_error("the answer"));
        }
        let mut status = 0u32;
        let mut len = 4u32;
        WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            &mut status as *mut u32 as *mut _,
            &mut len,
            null_mut(),
        );
        if status != 200 {
            return Err(format!("HTTP {status}"));
        }
        let mut body = Vec::new();
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let mut read = 0u32;
            if WinHttpReadData(req.0, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut read) == 0 {
                return Err(last_error("reading"));
            }
            if read == 0 {
                break;
            }
            body.extend_from_slice(&buf[..read as usize]);
            if body.len() as u64 > max_bytes {
                return Err("the file is bigger than expected".into());
            }
            progress(body.len() as u64);
        }
        Ok(body)
    }
}

/// SHA-256 of `data` (Windows' CNG).
pub fn sha256(data: &[u8]) -> [u8; 32] {
    use windows_sys::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
    let mut out = [0u8; 32];
    unsafe {
        BCryptHash(
            BCRYPT_SHA256_ALG_HANDLE,
            null(),
            0,
            data.as_ptr(),
            data.len() as u32,
            out.as_mut_ptr(),
            32,
        )
    };
    out
}

pub fn sha256_hex(data: &[u8]) -> String {
    sha256(data).iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_values() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn refuses_plain_http() {
        assert!(get("http://example.com/", "test", &[], 10, &mut |_| ()).is_err());
    }
}

/// Needs the internet: `cargo test -p pcm-win -- --ignored live`.
#[cfg(test)]
mod live {
    #[test]
    #[ignore]
    fn live_github_latest_release() {
        let url = "https://api.github.com/repos/vladZah403/pc-widget/releases/latest";
        let body = super::get(
            url,
            "PC-Monitor/test",
            &[("Accept", "application/vnd.github+json")],
            1 << 20,
            &mut |_| (),
        )
        .expect("GitHub answers");
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("\"tag_name\""), "{text}");
        println!("{}", &text[..text.len().min(300)]);
    }

    /// The latest release's zip through GitHub's redirect, checked against its digest, unpacked.
    #[test]
    #[ignore]
    fn live_download_latest_zip() {
        use pcm_core::update;
        let json = super::get(update::LATEST_API, "PC-Monitor/test", &[], 1 << 20, &mut |_| ()).unwrap();
        let info: serde_json::Value = serde_json::from_slice(&json).unwrap();
        let release = update::newer_release(&info, "0.1").expect("a release");
        let zip = release.zip.expect("its zip");
        let mut last = 0;
        let data = super::get(&zip.url, "PC-Monitor/test", &[], zip.size, &mut |n| last = n).unwrap();
        assert_eq!((data.len() as u64, last), (zip.size, zip.size));
        assert_eq!(super::sha256_hex(&data), zip.sha256);
        let dir = std::env::temp_dir().join(format!("pcm-live-{}", std::process::id()));
        update::unpack(&data, &dir.join("new"), &dir.join("app")).expect("unpacks");
        println!("{} {} bytes ok", release.version, data.len());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
