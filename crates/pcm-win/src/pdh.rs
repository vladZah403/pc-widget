//! Performance counters (PDH), by their English names so they work in any
//! Windows language.

use std::ptr::{null, null_mut};

use windows_sys::Win32::System::Performance::*;

use crate::{from_wide_ptr, wide};

pub struct Query {
    h: PDH_HQUERY,
}

/// A counter of a query; valid while the query is.
#[derive(Clone, Copy)]
pub struct Counter(PDH_HCOUNTER);

// SAFETY: PDH handles may be used from any thread (one at a time, which
// &mut / ownership guarantees for the readers that hold them).
unsafe impl Send for Query {}
unsafe impl Send for Counter {}

impl Query {
    pub fn open() -> Option<Query> {
        let mut h = null_mut();
        (unsafe { PdhOpenQueryW(null(), 0, &mut h) } == 0).then_some(Query { h })
    }

    /// "\Processor Information(_Total)\% Processor Utility"; None if this PC
    /// has no such counter.
    pub fn add(&self, path: &str) -> Option<Counter> {
        let mut c = null_mut();
        let p = wide(path);
        (unsafe { PdhAddEnglishCounterW(self.h, p.as_ptr(), 0, &mut c) } == 0).then_some(Counter(c))
    }

    /// Take a sample of every counter. Rates need two samples.
    pub fn collect(&self) -> bool {
        unsafe { PdhCollectQueryData(self.h) == 0 }
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.h) };
    }
}

fn valid(status: u32) -> bool {
    status == PDH_CSTATUS_VALID_DATA || status == PDH_CSTATUS_NEW_DATA
}

impl Counter {
    /// The value of a single-instance counter.
    pub fn value(&self) -> Option<f64> {
        let mut v = PDH_FMT_COUNTERVALUE::default();
        let rc = unsafe { PdhGetFormattedCounterValue(self.0, PDH_FMT_DOUBLE, null_mut(), &mut v) };
        (rc == 0 && valid(v.CStatus)).then_some(unsafe { v.Anonymous.doubleValue })
    }

    /// (instance name, value) of a wildcard counter ("...(*)\...").
    pub fn values(&self) -> Vec<(String, f64)> {
        let (mut size, mut count) = (0u32, 0u32);
        let rc = unsafe {
            PdhGetFormattedCounterArrayW(self.0, PDH_FMT_DOUBLE, &mut size, &mut count, null_mut())
        };
        if rc != PDH_MORE_DATA || size == 0 {
            return Vec::new();
        }
        // items followed by their names, in one buffer; u64 keeps it aligned
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
        let rc =
            unsafe { PdhGetFormattedCounterArrayW(self.0, PDH_FMT_DOUBLE, &mut size, &mut count, items) };
        if rc != 0 {
            return Vec::new();
        }
        (0..count as usize)
            .filter_map(|i| {
                let item = unsafe { &*items.add(i) };
                let name = unsafe { from_wide_ptr(item.szName) }?;
                valid(item.FmtValue.CStatus).then_some((name, unsafe { item.FmtValue.Anonymous.doubleValue }))
            })
            .collect()
    }
}
