//! A real ICMP ping through IcmpSendEcho (no admin rights needed).

use std::net::{SocketAddr, ToSocketAddrs};

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::NetworkManagement::IpHelper::{IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho};

pub struct Pinger(HANDLE);

// SAFETY: an ICMP handle may be used from any thread.
unsafe impl Send for Pinger {}

impl Pinger {
    pub fn new() -> Option<Pinger> {
        let h = unsafe { IcmpCreateFile() };
        (h != INVALID_HANDLE_VALUE && !h.is_null()).then_some(Pinger(h))
    }

    /// Round trip in ms to `host` (a name or IPv4), None if it doesn't answer
    /// within `timeout_ms` or can't be resolved.
    pub fn ping(&self, host: &str, timeout_ms: u32) -> Option<u32> {
        let ip = (host, 0).to_socket_addrs().ok()?.find_map(|a| match a {
            SocketAddr::V4(v4) => Some(*v4.ip()),
            SocketAddr::V6(_) => None,
        })?;
        let payload = *b"pc-monitor";
        let mut reply = [0u8; 256];
        let n = unsafe {
            IcmpSendEcho(
                self.0,
                u32::from_ne_bytes(ip.octets()),
                payload.as_ptr() as *const _,
                payload.len() as u16,
                std::ptr::null(),
                reply.as_mut_ptr() as *mut _,
                reply.len() as u32,
                timeout_ms,
            )
        };
        // ICMP_ECHO_REPLY: Address, Status, RoundTripTime
        let status = u32::from_ne_bytes(reply[4..8].try_into().ok()?);
        let rtt = u32::from_ne_bytes(reply[8..12].try_into().ok()?);
        (n > 0 && status == 0).then_some(rtt)
    }
}

impl Drop for Pinger {
    fn drop(&mut self) {
        unsafe { IcmpCloseHandle(self.0) };
    }
}
