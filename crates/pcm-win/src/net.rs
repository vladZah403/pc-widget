//! Network speed from the interface counters (physical adapters only, so a
//! VPN or a filter driver doesn't count the same bytes twice) and the main
//! adapter's name and IPv4 address.

use std::ptr::null_mut;

use pcm_core::model::Net;
use pcm_core::rate::Rate;
use windows_sys::Win32::NetworkManagement::IpHelper::*;

use crate::{from_wide_ptr, now_s};

const IF_OPER_UP: i32 = 1;
const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
const AF_INET: u16 = 2;

/// Physical adapters that are up: (LUID, received bytes, sent bytes). Not
/// VPNs, tunnels or filter drivers, which see the same bytes again.
fn physical() -> Option<Vec<(u64, u64, u64)>> {
    let mut table: *mut MIB_IF_TABLE2 = null_mut();
    if unsafe { GetIfTable2(&mut table) } != 0 || table.is_null() {
        return None;
    }
    let mut out = Vec::new();
    unsafe {
        let n = (*table).NumEntries as usize;
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), n);
        for r in rows {
            let flags = r.InterfaceAndOperStatusFlags._bitfield;
            let hardware = flags & 0x1 != 0;
            let filter = flags & 0x2 != 0;
            if hardware && !filter && r.OperStatus == IF_OPER_UP && r.Type != IF_TYPE_SOFTWARE_LOOPBACK {
                out.push((r.InterfaceLuid.Value, r.InOctets, r.OutOctets));
            }
        }
        FreeMibTable(table as *const _);
    }
    Some(out)
}

/// The physical adapter (of `luids`) with an IPv4 default gateway, the
/// lowest metric winning: its name ("Wi-Fi") and IPv4 address.
fn main_adapter(luids: &[u64]) -> Option<(String, Option<String>)> {
    let flags = GAA_FLAG_INCLUDE_GATEWAYS
        | GAA_FLAG_SKIP_ANYCAST
        | GAA_FLAG_SKIP_MULTICAST
        | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size = 16 * 1024u32;
    let mut buf: Vec<u64>;
    loop {
        buf = vec![0u64; (size as usize).div_ceil(8)];
        let rc = unsafe { GetAdaptersAddresses(0, flags, null_mut(), buf.as_mut_ptr() as *mut _, &mut size) };
        match rc {
            0 => break,
            111 => continue, // ERROR_BUFFER_OVERFLOW: size now holds what is needed
            _ => return None,
        }
    }
    let mut best: Option<(u32, String, Option<String>)> = None;
    let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    while !p.is_null() {
        let a = unsafe { &*p };
        p = a.Next;
        // an IPv4 default gateway: Teredo and other tunnels have only IPv6 ones
        let mut gw = a.FirstGatewayAddress;
        let mut ipv4_gateway = false;
        while !gw.is_null() {
            let g = unsafe { &*gw };
            gw = g.Next;
            let sa = g.Address.lpSockaddr as *const u16;
            if !sa.is_null() && unsafe { *sa } == AF_INET {
                ipv4_gateway = true;
            }
        }
        if a.OperStatus != IF_OPER_UP || !ipv4_gateway || !luids.contains(&unsafe { a.Luid.Value }) {
            continue;
        }
        let name = unsafe { from_wide_ptr(a.FriendlyName) }.unwrap_or_default();
        let mut ipv4 = None;
        let mut u = a.FirstUnicastAddress;
        while !u.is_null() {
            let addr = unsafe { &*u };
            u = addr.Next;
            let sa = addr.Address.lpSockaddr as *const u8;
            // SOCKADDR_IN: family (u16), port (u16), address (4 bytes)
            if !sa.is_null() && unsafe { *(sa as *const u16) } == AF_INET {
                let b = unsafe { std::slice::from_raw_parts(sa.add(4), 4) };
                ipv4 = Some(format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]));
                break;
            }
        }
        if best.as_ref().is_none_or(|(m, _, _)| a.Ipv4Metric < *m) {
            best = Some((a.Ipv4Metric, name, ipv4));
        }
    }
    best.map(|(_, n, ip)| (n, ip))
}

/// Received and sent bytes of these adapters, None if one has gone or is down.
fn counters(luids: &[u64]) -> Option<(u64, u64)> {
    let (mut down, mut up) = (0u64, 0u64);
    for &luid in luids {
        let mut row: MIB_IF_ROW2 = unsafe { std::mem::zeroed() };
        row.InterfaceLuid.Value = luid;
        if unsafe { GetIfEntry2(&mut row) } != 0 || row.OperStatus != IF_OPER_UP {
            return None;
        }
        down += row.InOctets;
        up += row.OutOctets;
    }
    Some((down, up))
}

/// The adapters are looked up again this often: the full table (every
/// virtual adapter and filter, ~100 rows) and the addresses cost ~2 ms,
/// a row each second ~0.05 ms.
const ADAPTERS_EVERY_S: f64 = 30.0;

struct Adapters {
    at: f64,
    luids: Vec<u64>,
    name: Option<String>,
    ipv4: Option<String>,
}

#[derive(Default)]
pub struct NetReader {
    down: Rate,
    up: Rate,
    adapters: Option<Adapters>,
}

impl NetReader {
    /// Speeds since the last call (None on the first one).
    pub fn read(&mut self) -> Option<Net> {
        let t = now_s();
        let mut sums =
            self.adapters.as_ref().filter(|a| t - a.at < ADAPTERS_EVERY_S).and_then(|a| counters(&a.luids));
        if sums.is_none() {
            // first, due, or an adapter went away / came up: look again
            let list = physical()?;
            let luids: Vec<u64> = list.iter().map(|a| a.0).collect();
            if self.adapters.as_ref().is_none_or(|a| a.luids != luids) {
                // other adapters, other counters: no speed from the old ones
                (self.down, self.up) = (Rate::default(), Rate::default());
            }
            let (name, ipv4) = main_adapter(&luids).map_or((None, None), |(n, ip)| (Some(n), ip));
            sums = Some((list.iter().map(|a| a.1).sum(), list.iter().map(|a| a.2).sum()));
            self.adapters = Some(Adapters { at: t, luids, name, ipv4 });
        }
        let (down_total, up_total) = sums?;
        let down = self.down.update(t, down_total);
        let up = self.up.update(t, up_total);
        let a = self.adapters.as_ref()?;
        Some(Net { down_bps: down?, up_bps: up?, adapter: a.name.clone(), ipv4: a.ipv4.clone() })
    }
}
