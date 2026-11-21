//! Video adapters: names, dedicated memory and temperature from the kernel
//! graphics interface (D3DKMT, as Task Manager uses — no admin rights), load
//! and memory in use from the "GPU Engine" / "GPU Adapter Memory" counters.

use std::ptr::null_mut;

use pcm_core::gpu_engines;
use pcm_core::model::{Gpu, Temp};
use pcm_core::sensors::is_integrated;
use windows_sys::Wdk::Graphics::Direct3D::*;

use crate::from_wide;
use crate::pdh::{Counter, Query};

struct Adapter {
    handle: u32,
    luid: u64,
    name: String,
    dedicated_total: Option<u64>,
}

fn query<T>(handle: u32, kind: KMTQUERYADAPTERINFOTYPE, out: &mut T) -> bool {
    let mut q = D3DKMT_QUERYADAPTERINFO {
        hAdapter: handle,
        Type: kind,
        pPrivateDriverData: out as *mut T as *mut _,
        PrivateDriverDataSize: size_of::<T>() as u32,
    };
    unsafe { D3DKMTQueryAdapterInfo(&mut q) >= 0 }
}

fn close(handle: u32) {
    unsafe { D3DKMTCloseAdapter(&D3DKMT_CLOSEADAPTER { hAdapter: handle }) };
}

/// Every real adapter (not the "Microsoft Basic Render Driver"), opened.
fn adapters() -> Vec<Adapter> {
    let mut e = D3DKMT_ENUMADAPTERS2 { NumAdapters: 0, pAdapters: null_mut() };
    if unsafe { D3DKMTEnumAdapters2(&mut e) } < 0 || e.NumAdapters == 0 {
        return Vec::new();
    }
    let mut list: Vec<D3DKMT_ADAPTERINFO> = vec![unsafe { std::mem::zeroed() }; e.NumAdapters as usize];
    e.pAdapters = list.as_mut_ptr();
    if unsafe { D3DKMTEnumAdapters2(&mut e) } < 0 {
        return Vec::new();
    }
    list.truncate(e.NumAdapters as usize);
    let mut out = Vec::new();
    for a in list {
        let mut desc: Box<D3DKMT_DRIVER_DESCRIPTION> = Box::new(unsafe { std::mem::zeroed() });
        let name = if query(a.hAdapter, KMTQAITYPE_DRIVER_DESCRIPTION, &mut *desc) {
            from_wide(&desc.DriverDescription).trim().to_string()
        } else {
            String::new()
        };
        if name.is_empty()
            || name.contains("Microsoft Basic Render")
            || name.contains("Microsoft Basic Display")
        {
            close(a.hAdapter);
            continue;
        }
        let mut seg: D3DKMT_SEGMENTSIZEINFO = unsafe { std::mem::zeroed() };
        let dedicated_total = query(a.hAdapter, KMTQAITYPE_GETSEGMENTSIZE, &mut seg)
            .then_some(seg.DedicatedVideoMemorySize)
            .filter(|&v| v > 0);
        let luid = (a.AdapterLuid.HighPart as u32 as u64) << 32 | a.AdapterLuid.LowPart as u64;
        // one physical GPU can be listed twice (render + display); keep the first
        if out.iter().any(|x: &Adapter| x.luid == luid) {
            close(a.hAdapter);
            continue;
        }
        out.push(Adapter { handle: a.hAdapter, luid, name, dedicated_total });
    }
    out
}

pub struct GpuReader {
    adapters: Vec<Adapter>,
    query: Option<Query>,
    engines: Option<Counter>,
    dedicated: Option<Counter>,
    shared: Option<Counter>,
}

impl GpuReader {
    pub fn new() -> GpuReader {
        let query = Query::open();
        let add = |path: &str| query.as_ref().and_then(|q| q.add(path));
        let engines = add(r"\GPU Engine(*)\Utilization Percentage");
        let dedicated = add(r"\GPU Adapter Memory(*)\Dedicated Usage");
        let shared = add(r"\GPU Adapter Memory(*)\Shared Usage");
        if let Some(q) = &query {
            q.collect();
        }
        GpuReader { adapters: adapters(), query, engines, dedicated, shared }
    }

    pub fn read(&mut self) -> Vec<Gpu> {
        let collected = self.query.as_ref().is_some_and(|q| q.collect());
        let values = |c: Option<Counter>| {
            if collected { c.map(|c| c.values()).unwrap_or_default() } else { Vec::new() }
        };
        let loads = gpu_engines::loads(&values(self.engines));
        let dedicated = gpu_engines::memory(&values(self.dedicated));
        let shared = gpu_engines::memory(&values(self.shared));
        self.adapters
            .iter()
            .map(|a| {
                let mut perf: D3DKMT_ADAPTER_PERFDATA = unsafe { std::mem::zeroed() };
                let perf_ok = query(a.handle, KMTQAITYPE_ADAPTERPERFDATA, &mut perf);
                // deci-°C; drivers without a sensor leave 0
                let temp = (perf_ok && perf.Temperature > 0)
                    .then(|| perf.Temperature as f32 / 10.0)
                    .filter(|t| *t > 1.0 && *t < 150.0)
                    .map(Temp::exact);
                Gpu {
                    name: a.name.clone(),
                    luid: a.luid,
                    integrated: is_integrated(&a.name),
                    load: collected.then(|| loads.get(&a.luid).map_or(0.0, |l| l.load)),
                    dedicated_used: dedicated.get(&a.luid).copied(),
                    dedicated_total: a.dedicated_total,
                    shared_used: shared.get(&a.luid).copied(),
                    temp,
                    clock_mhz: None,
                    fan_rpm: (perf_ok && perf.FanRPM > 0).then_some(perf.FanRPM),
                }
            })
            .collect()
    }
}

impl Default for GpuReader {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for GpuReader {
    fn drop(&mut self) {
        for a in &self.adapters {
            close(a.handle);
        }
    }
}
