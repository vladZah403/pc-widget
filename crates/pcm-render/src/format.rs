//! Numbers as the widget writes them, in 1.8's words. Russian is the source
//! language (as in 1.8): `tr` will look up the other languages in stage 5.

/// The interface text in the current language (Russian for now).
pub fn tr(text: &str) -> String {
    text.to_string()
}

/// `tr` with "{name}" fields filled in.
pub fn trf(text: &str, fields: &[(&str, String)]) -> String {
    let mut s = tr(text);
    for (k, v) in fields {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

/// Decimal comma (Russian, Ukrainian, German).
pub fn dec(s: String) -> String {
    s.replace('.', ",")
}

pub fn fmt_gb(bytes: u64) -> String {
    dec(format!("{:.1}", bytes as f64 / (1u64 << 30) as f64))
}

/// "300,0 из 476,0 ГБ"; 1 TB and more in TB.
pub fn fmt_disk(used: u64, total: u64) -> String {
    if total >= 1 << 40 {
        let tb = |n: u64| dec(format!("{:.1}", n as f64 / (1u64 << 40) as f64));
        return trf("{used} из {total} ТБ", &[("used", tb(used)), ("total", tb(total))]);
    }
    trf("{used} из {total} ГБ", &[("used", fmt_gb(used)), ("total", fmt_gb(total))])
}

pub fn fmt_speed(bps: f64) -> String {
    if bps < 1024.0 {
        trf("{v} Б/с", &[("v", format!("{:.0}", bps.max(0.0)))])
    } else if bps < 1024.0 * 1024.0 {
        trf("{v} КБ/с", &[("v", format!("{:.0}", bps / 1024.0))])
    } else {
        trf("{v} МБ/с", &[("v", dec(format!("{:.1}", bps / 1048576.0)))])
    }
}

pub fn fmt_mem(bytes: u64) -> String {
    if bytes < 1 << 30 {
        trf("{v} МБ", &[("v", format!("{:.0}", bytes as f64 / 1048576.0))])
    } else {
        trf("{v} ГБ", &[("v", dec(format!("{:.1}", bytes as f64 / (1u64 << 30) as f64)))])
    }
}

pub fn fmt_wh(wh: f32) -> String {
    dec(format!("{wh:.1}"))
}

/// Battery time left: "2 ч 15 мин", "45 мин".
pub fn fmt_left(seconds: u32) -> String {
    let minutes = seconds / 60;
    let (h, m) = (minutes / 60, minutes % 60);
    if h > 0 {
        trf("{h} ч {m} мин", &[("h", h.to_string()), ("m", m.to_string())])
    } else {
        trf("{m} мин", &[("m", m.to_string())])
    }
}

pub fn fmt_uptime(seconds: u64) -> String {
    let (d, rest) = (seconds / 86400, seconds % 86400);
    let (h, m) = (rest / 3600, rest % 3600 / 60);
    if d > 0 {
        trf("{d} д {h} ч", &[("d", d.to_string()), ("h", h.to_string())])
    } else {
        trf("{h} ч {m} мин", &[("h", h.to_string()), ("m", m.to_string())])
    }
}

/// Drive makers count endurance in decimal terabytes.
pub fn fmt_written(bytes: u64) -> String {
    if bytes as f64 >= 1e12 {
        trf("{v} ТБ", &[("v", dec(format!("{:.1}", bytes as f64 / 1e12)))])
    } else {
        trf("{v} ГБ", &[("v", format!("{:.0}", bytes as f64 / 1e9))])
    }
}

/// "3,2 ГГц  ·  12 Вт" from the sensors, or "".
pub fn cpu_extra(clock_ghz: Option<f32>, power_w: Option<f32>) -> String {
    let mut parts = Vec::new();
    if let Some(c) = clock_ghz.filter(|c| *c > 0.0) {
        parts.push(trf("{v} ГГц", &[("v", dec(format!("{c:.1}")))]));
    }
    if let Some(p) = power_w.filter(|p| *p > 0.0) {
        let v = if p >= 10.0 { format!("{p:.0}") } else { dec(format!("{p:.1}")) };
        parts.push(trf("{v} Вт", &[("v", v)]));
    }
    parts.join("  ·  ")
}

fn words_without(name: &str, drop: &[&str]) -> String {
    name.split_whitespace()
        .filter(|w| !drop.iter().any(|d| w.eq_ignore_ascii_case(d)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// "AMD Ryzen 5 5500U with Radeon Graphics" -> "Ryzen 5 5500U";
/// "Intel(R) Core(TM) i5-10400F CPU @ 2.90GHz" -> "Core i5-10400F".
pub fn short_cpu_name(name: &str) -> String {
    let mut s = name.replace("(R)", "").replace("(TM)", "").replace("(tm)", "");
    if let Some(i) = s.find(" @") {
        s.truncate(i);
    }
    let low = s.to_lowercase();
    for cut in [" with radeon", " w/ radeon"] {
        if let Some(i) = low.find(cut) {
            s.truncate(i);
            break;
        }
    }
    let words: Vec<&str> = s.split_whitespace().collect();
    let kept: Vec<&str> = words
        .into_iter()
        .filter(|w| {
            let l = w.to_lowercase();
            !(["amd", "intel", "cpu", "processor"].contains(&l.as_str())
                || l.ends_with("-core")
                || (l.ends_with("th") || l.ends_with("st") || l.ends_with("nd") || l.ends_with("rd"))
                    && l.chars().next().is_some_and(|c| c.is_ascii_digit()))
                && l != "gen"
        })
        .collect();
    kept.join(" ")
}

/// "NVIDIA GeForce RTX 3060" -> "GeForce RTX 3060"; "AMD Radeon(TM) Graphics"
/// -> "Radeon Graphics"; the maker stays when only "Graphics" would be left.
pub fn short_gpu_name(name: &str) -> String {
    let s = words_without(
        &name.replace("(TM)", "").replace("(tm)", "").replace("(R)", "").replace("(r)", ""),
        &[],
    );
    let rest = words_without(&s, &["AMD", "NVIDIA", "Intel"]);
    if rest.eq_ignore_ascii_case("graphics") || rest.is_empty() { s } else { rest }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_like_1_8() {
        assert_eq!(fmt_disk(300 << 30, 476 << 30), "300,0 из 476,0 ГБ");
        assert_eq!(fmt_disk(1 << 40, 2 << 40), "1,0 из 2,0 ТБ");
        assert_eq!(fmt_speed(500.0), "500 Б/с");
        assert_eq!(fmt_speed(120.0 * 1024.0), "120 КБ/с");
        assert_eq!(fmt_speed(2.5 * 1048576.0), "2,5 МБ/с");
        assert_eq!(fmt_mem(300 << 20), "300 МБ");
        assert_eq!(fmt_left(2 * 3600 + 15 * 60), "2 ч 15 мин");
        assert_eq!(fmt_left(45 * 60), "45 мин");
        assert_eq!(fmt_uptime(3 * 3600 + 12 * 60), "3 ч 12 мин");
        assert_eq!(fmt_uptime(2 * 86400 + 5 * 3600), "2 д 5 ч");
        assert_eq!(fmt_written(38_900_000_000_000), "38,9 ТБ");
        assert_eq!(cpu_extra(Some(3.21), Some(12.4)), "3,2 ГГц  ·  12 Вт");
        assert_eq!(cpu_extra(None, Some(8.25)), "8,2 Вт");
    }

    #[test]
    fn short_names() {
        assert_eq!(short_cpu_name("AMD Ryzen 5 5500U with Radeon Graphics"), "Ryzen 5 5500U");
        assert_eq!(short_cpu_name("Intel(R) Core(TM) i5-10400F CPU @ 2.90GHz"), "Core i5-10400F");
        assert_eq!(short_cpu_name("12th Gen Intel(R) Core(TM) i7-12700H"), "Core i7-12700H");
        assert_eq!(short_cpu_name("AMD Ryzen 7 5800X 8-Core Processor"), "Ryzen 7 5800X");
        assert_eq!(short_gpu_name("NVIDIA GeForce RTX 3060"), "GeForce RTX 3060");
        assert_eq!(short_gpu_name("AMD Radeon(TM) Graphics"), "Radeon Graphics");
        assert_eq!(short_gpu_name("Intel(R) Graphics"), "Intel Graphics");
    }
}
