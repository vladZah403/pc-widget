//! What went wrong, from the Windows event logs: programs that crashed or
//! hung, blue screens, sudden power losses, graphics driver resets, the
//! firmware holding the processor back and hardware errors (WHEA). Readable
//! without admin rights.

use std::ptr::null;

use windows_sys::Win32::System::EventLog::{
    EVT_HANDLE, EvtClose, EvtNext, EvtQuery, EvtQueryChannelPath, EvtQueryReverseDirection,
    EvtQueryTolerateQueryErrors, EvtRender, EvtRenderEventXml,
};

use crate::wide;

/// What kind of trouble.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A program crashed (Application Error 1000); detail: the program.
    Crash,
    /// A program stopped responding and was closed (Application Hang 1002).
    Hang,
    /// A blue screen (BugCheck 1001); detail: its code.
    BlueScreen,
    /// Power lost or the PC froze and was restarted (Kernel-Power 41).
    PowerLoss,
    /// The graphics driver stopped responding and recovered (Display 4101); detail: the driver.
    DriverReset,
    /// The firmware limits the processor's speed (Kernel-Processor-Power 37).
    Firmware,
    /// The processor, memory or a bus reported a hardware error (WHEA-Logger).
    Hardware,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// Unix seconds.
    pub time: i64,
    pub kind: Kind,
    /// The program, the driver or the code ("Terraria.exe", "amdkmdag", "0x0000009f").
    pub detail: String,
}

/// The last `days` days of trouble, newest first (at most `max`).
pub fn recent(days: u32, max: usize) -> Vec<Event> {
    let ms = days as u64 * 86_400_000;
    let time = format!("TimeCreated[timediff(@SystemTime) <= {ms}]");
    let system = format!(
        "*[System[{time} and (\
         (Provider[@Name='Microsoft-Windows-Kernel-Power'] and EventID=41) or \
         (Provider[@Name='Microsoft-Windows-WER-SystemErrorReporting'] and EventID=1001) or \
         (Provider[@Name='Display'] and EventID=4101) or \
         (Provider[@Name='Microsoft-Windows-Kernel-Processor-Power'] and EventID=37) or \
         (Provider[@Name='Microsoft-Windows-WHEA-Logger'] and (EventID=17 or EventID=18 or EventID=19 or EventID=47)))]]"
    );
    let application = format!(
        "*[System[{time} and (\
         (Provider[@Name='Application Error'] and EventID=1000) or \
         (Provider[@Name='Application Hang'] and EventID=1002))]]"
    );
    let mut out: Vec<Event> =
        query("System", &system, max).into_iter().chain(query("Application", &application, max)).collect();
    out.sort_by_key(|e| std::cmp::Reverse(e.time));
    out.truncate(max);
    out
}

/// The events of `channel` matching `xpath`, newest first, as parsed.
fn query(channel: &str, xpath: &str, max: usize) -> Vec<Event> {
    let mut out = Vec::new();
    unsafe {
        let (path, q) = (wide(channel), wide(xpath));
        let flags = EvtQueryChannelPath | EvtQueryReverseDirection | EvtQueryTolerateQueryErrors;
        let rs: EVT_HANDLE = EvtQuery(0, path.as_ptr(), q.as_ptr(), flags);
        if rs == 0 {
            return out;
        }
        let mut handles = [0isize; 32];
        'all: loop {
            let mut got = 0u32;
            if EvtNext(rs, handles.len() as u32, handles.as_mut_ptr(), 1000, 0, &mut got) == 0 || got == 0 {
                break;
            }
            for h in &handles[..got as usize] {
                if let Some(e) = render(*h).and_then(|xml| parse(&xml)) {
                    out.push(e);
                }
                EvtClose(*h);
            }
            if out.len() >= max {
                break 'all;
            }
        }
        EvtClose(rs);
    }
    out
}

/// One event as its XML.
unsafe fn render(h: EVT_HANDLE) -> Option<String> {
    let (mut used, mut props) = (0u32, 0u32);
    unsafe {
        EvtRender(0, h, EvtRenderEventXml, 0, null::<u8>() as _, &mut used, &mut props);
        if used == 0 {
            return None;
        }
        let mut buf = vec![0u16; used as usize / 2 + 1];
        if EvtRender(
            0,
            h,
            EvtRenderEventXml,
            (buf.len() * 2) as u32,
            buf.as_mut_ptr() as _,
            &mut used,
            &mut props,
        ) == 0
        {
            return None;
        }
        let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..n]))
    }
}

/// The text of the first `<tag ...>text</tag>` after `from`, and where it ends.
fn element<'a>(xml: &'a str, tag: &str, from: usize) -> Option<(&'a str, usize)> {
    let open = xml[from..].find(&format!("<{tag}"))? + from;
    let gt = xml[open..].find('>')? + open;
    if xml[..gt].ends_with('/') {
        return Some(("", gt + 1));
    }
    let close = xml[gt..].find(&format!("</{tag}>"))? + gt;
    Some((&xml[gt + 1..close], close))
}

/// An attribute's value in the first `<tag ...>`.
fn attr<'a>(xml: &'a str, tag: &str, name: &str) -> Option<&'a str> {
    let open = xml.find(&format!("<{tag} "))?;
    let end = xml[open..].find('>')? + open;
    let head = &xml[open..end];
    let at = head.find(&format!("{name}="))? + name.len() + 1;
    let quote = head[at..].chars().next()?;
    let rest = &head[at + 1..];
    Some(&rest[..rest.find(quote)?])
}

/// The `<Data>` values, in order.
fn data(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = xml.find("<EventData").unwrap_or(xml.len());
    while let Some((text, end)) = element(xml, "Data", at) {
        out.push(unescape(text.trim()));
        at = end;
    }
    out
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// "2026-11-23T20:12:33.1234567Z" as unix seconds.
fn unix_of(t: &str) -> Option<i64> {
    let n = |a: usize, b: usize| t.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, s) = (n(0, 4)?, n(5, 7)?, n(8, 10)?, n(11, 13)?, n(14, 16)?, n(17, 19)?);
    // days from the civil date (Howard Hinnant's algorithm)
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + s)
}

/// An event's XML as one of ours (None: not one we know).
fn parse(xml: &str) -> Option<Event> {
    let provider = attr(xml, "Provider", "Name")?;
    let id: u32 = element(xml, "EventID", 0)?.0.trim().parse().ok()?;
    let time = unix_of(attr(xml, "TimeCreated", "SystemTime")?)?;
    let d = data(xml);
    let first = d.first().cloned().unwrap_or_default();
    let (kind, detail) = match (provider, id) {
        ("Application Error", 1000) => (Kind::Crash, first),
        ("Application Hang", 1002) => (Kind::Hang, first),
        ("Microsoft-Windows-WER-SystemErrorReporting", 1001) => {
            // "0x0000009f (0x..., ...)": the code
            (Kind::BlueScreen, first.split_whitespace().next().unwrap_or("").to_string())
        }
        ("Microsoft-Windows-Kernel-Power", 41) => (Kind::PowerLoss, String::new()),
        ("Display", 4101) => (Kind::DriverReset, first),
        ("Microsoft-Windows-Kernel-Processor-Power", 37) => (Kind::Firmware, String::new()),
        ("Microsoft-Windows-WHEA-Logger", _) => (Kind::Hardware, String::new()),
        _ => return None,
    };
    Some(Event { time, kind, detail })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRASH: &str = "<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System>\
        <Provider Name='Application Error'/><EventID Qualifiers='0'>1000</EventID>\
        <TimeCreated SystemTime='2026-11-23T20:12:33.1234567Z'/></System><EventData>\
        <Data>Terraria.exe</Data><Data>1.4.4.9</Data><Data>65a0b1c2</Data></EventData></Event>";

    #[test]
    fn a_crash() {
        let e = parse(CRASH).unwrap();
        assert_eq!(e.kind, Kind::Crash);
        assert_eq!(e.detail, "Terraria.exe");
        // 2026-11-23 20:12:33 UTC
        assert_eq!(e.time, 1_795_464_753);
    }

    #[test]
    fn a_blue_screen_and_a_power_loss() {
        let bsod = "<Event><System><Provider Name='Microsoft-Windows-WER-SystemErrorReporting' Guid='{x}' \
            EventSourceName='BugCheck'/><EventID Qualifiers='16384'>1001</EventID>\
            <TimeCreated SystemTime='2026-01-02T03:04:05.000Z'/></System><EventData>\
            <Data Name='param1'>0x0000009f (0x0000000000000003, 0xffff)</Data></EventData></Event>";
        let e = parse(bsod).unwrap();
        assert_eq!((e.kind, e.detail.as_str()), (Kind::BlueScreen, "0x0000009f"));
        let power = "<Event><System><Provider Name=\"Microsoft-Windows-Kernel-Power\" Guid=\"{y}\"/>\
            <EventID>41</EventID><TimeCreated SystemTime=\"2026-01-02T03:04:05Z\"/></System>\
            <EventData><Data Name=\"BugcheckCode\">0</Data></EventData></Event>";
        assert_eq!(parse(power).unwrap().kind, Kind::PowerLoss);
        assert!(parse(&power.replace(">41<", ">42<")).is_none(), "not ours");
    }

    #[test]
    fn dates() {
        assert_eq!(unix_of("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(unix_of("2000-03-01T00:00:00Z"), Some(951_868_800));
    }

    #[test]
    fn reads_the_logs() {
        // whatever this PC has: it must not fail or take long
        let t = std::time::Instant::now();
        let list = recent(30, 200);
        assert!(t.elapsed().as_secs() < 10);
        assert!(list.windows(2).all(|w| w[0].time >= w[1].time), "newest first");
        eprintln!("{} events in {:?}", list.len(), t.elapsed());
        for e in list.iter().take(8) {
            eprintln!("{e:?}");
        }
    }
}
