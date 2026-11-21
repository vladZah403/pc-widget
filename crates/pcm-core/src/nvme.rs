//! The NVMe "SMART / Health Information" log page (log id 2), as 1.8's
//! `parse_nvme_health` reads it. Windows hands it out without admin rights.

/// What the log says; the drive number, model and letters come from elsewhere.
#[derive(Clone, Debug, PartialEq)]
pub struct HealthLog {
    pub critical: u8,
    pub temp: Option<f32>,
    pub spare: u8,
    pub spare_min: u8,
    pub wear: u8,
    pub written: u64,
    pub hours: u64,
}

fn u128_le(b: &[u8]) -> u128 {
    let mut a = [0u8; 16];
    a.copy_from_slice(&b[..16]);
    u128::from_le_bytes(a)
}

pub fn parse_health(log: &[u8]) -> Option<HealthLog> {
    if log.len() < 144 {
        return None;
    }
    let kelvin = u16::from_le_bytes([log[1], log[2]]) as i32;
    Some(HealthLog {
        critical: log[0],
        temp: (273 < kelvin && kelvin < 273 + 150).then(|| (kelvin - 273) as f32),
        spare: log[3],
        spare_min: log[4],
        wear: log[5].min(100),
        // units of 1000 sectors of 512 bytes
        written: u128_le(&log[48..64]).saturating_mul(512_000).min(u64::MAX as u128) as u64,
        hours: u128_le(&log[128..144]).min(u64::MAX as u128) as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_log_fields() {
        let mut log = vec![0u8; 512];
        log[0] = 0;
        log[1..3].copy_from_slice(&(273u16 + 32).to_le_bytes());
        log[3] = 100;
        log[4] = 10;
        log[5] = 3;
        // 38.9 TB written = 75_976_562 units of 512 000 bytes
        log[48..56].copy_from_slice(&75_976_562u64.to_le_bytes());
        log[128..136].copy_from_slice(&2101u64.to_le_bytes());
        let h = parse_health(&log).unwrap();
        assert_eq!(h.temp, Some(32.0));
        assert_eq!((h.spare, h.spare_min, h.wear, h.hours), (100, 10, 3, 2101));
        assert_eq!(h.written, 75_976_562 * 512_000);
        assert!(parse_health(&log[..100]).is_none());
        log[1..3].copy_from_slice(&0u16.to_le_bytes());
        log[5] = 255;
        let h = parse_health(&log).unwrap();
        assert_eq!((h.temp, h.wear), (None, 100), "no sensor; wear can pass 100 but is capped");
    }
}
