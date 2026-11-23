//! Watches what Windows says limits the CPU, once a second:
//! `cargo run -p pcm-win --example perf_limit -- [seconds]`

use pcm_win::pdh::Query;

fn main() {
    let secs: u32 = std::env::args().nth(1).and_then(|v| v.parse().ok()).unwrap_or(5);
    let q = Query::open().expect("pdh");
    let names = [
        r"\Processor Information(_Total)\% Performance Limit",
        r"\Processor Information(_Total)\Performance Limit Flags",
        r"\Processor Information(_Total)\% Processor Performance",
        r"\Processor Information(_Total)\Processor Frequency",
        r"\Processor Information(_Total)\% Processor Utility",
    ];
    let counters: Vec<_> = names.iter().map(|n| (n, q.add(n))).collect();
    for (n, c) in &counters {
        if c.is_none() {
            println!("missing: {n}");
        }
    }
    q.collect();
    for _ in 0..secs {
        std::thread::sleep(std::time::Duration::from_secs(1));
        q.collect();
        let line: Vec<String> = counters
            .iter()
            .filter_map(|(n, c)| {
                let v = c.as_ref()?.value()?;
                Some(format!("{}={v:.1}", n.rsplit('\\').next().unwrap_or(n)))
            })
            .collect();
        println!("{}", line.join("  "));
    }
}
