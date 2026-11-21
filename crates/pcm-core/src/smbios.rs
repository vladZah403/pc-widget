//! SMBIOS tables (from GetSystemFirmwareTable('RSMB')): the board, the BIOS
//! and the RAM sticks — no WMI, no admin rights.

use crate::model::{Board, MemoryModule};

/// One structure: its type, the formatted part and its strings.
struct Structure<'a> {
    kind: u8,
    data: &'a [u8],
    strings: Vec<&'a [u8]>,
}

impl Structure<'_> {
    fn byte(&self, off: usize) -> Option<u8> {
        self.data.get(off).copied()
    }

    fn word(&self, off: usize) -> Option<u16> {
        Some(u16::from_le_bytes([*self.data.get(off)?, *self.data.get(off + 1)?]))
    }

    fn dword(&self, off: usize) -> Option<u32> {
        let b = self.data.get(off..off + 4)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// The string a byte at `off` points to (1-based; 0 = none), trimmed;
    /// placeholders like "To Be Filled By O.E.M." count as none.
    fn string(&self, off: usize) -> Option<String> {
        let i = self.byte(off)? as usize;
        let s = String::from_utf8_lossy(self.strings.get(i.checked_sub(1)?)?).trim().to_string();
        let low = s.to_ascii_lowercase();
        let junk = s.is_empty()
            || low.contains("to be filled")
            || low == "default string"
            || low == "not specified"
            || low == "system product name"
            || low == "system manufacturer"
            || low.starts_with("unknown");
        (!junk).then_some(s)
    }
}

/// The structures of a RawSMBIOSData buffer (8-byte header + table).
fn structures(raw: &[u8]) -> Vec<Structure<'_>> {
    let mut out = Vec::new();
    let Some(len) = raw.get(4..8).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize) else {
        return out;
    };
    let table = &raw[8..raw.len().min(8 + len)];
    let mut pos = 0;
    while pos + 4 <= table.len() {
        let kind = table[pos];
        let flen = table[pos + 1] as usize;
        if flen < 4 || pos + flen > table.len() {
            break;
        }
        let data = &table[pos..pos + flen];
        // strings follow, each ending in 0, the set ending in another 0
        let mut strings = Vec::new();
        let mut p = pos + flen;
        if table.get(p..p + 2) == Some(&[0, 0]) {
            p += 2;
        } else {
            loop {
                let Some(end) = table[p..].iter().position(|&b| b == 0) else {
                    return out;
                };
                strings.push(&table[p..p + end]);
                p += end + 1;
                if table.get(p) == Some(&0) {
                    p += 1;
                    break;
                }
            }
        }
        out.push(Structure { kind, data, strings });
        if kind == 127 {
            break; // end of table
        }
        pos = p;
    }
    out
}

fn memory_kind(code: u8) -> Option<&'static str> {
    Some(match code {
        0x12 => "DDR",
        0x13 => "DDR2",
        0x18 => "DDR3",
        0x1A => "DDR4",
        0x1B => "LPDDR",
        0x1C => "LPDDR2",
        0x1D => "LPDDR3",
        0x1E => "LPDDR4",
        0x22 => "DDR5",
        0x23 => "LPDDR5",
        _ => return None,
    })
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tables {
    pub board: Board,
    pub memory: Vec<MemoryModule>,
}

pub fn parse(raw: &[u8]) -> Tables {
    let mut t = Tables::default();
    for s in structures(raw) {
        match s.kind {
            0 => {
                t.board.bios_vendor = s.string(0x04);
                t.board.bios_version = s.string(0x05);
                t.board.bios_date = s.string(0x08);
            }
            1 => {
                t.board.system_maker = s.string(0x04);
                t.board.system_product = s.string(0x05);
            }
            2 => {
                t.board.board_maker = s.string(0x04);
                t.board.board_product = s.string(0x05);
            }
            17 => {
                // size: 0 = empty slot; 0x7FFF = see extended size (MB); bit 15 = KB
                let size = match s.word(0x0C) {
                    None | Some(0) | Some(0xFFFF) => continue,
                    Some(0x7FFF) => s.dword(0x1C).unwrap_or(0) as u64 * 1024 * 1024,
                    Some(w) if w & 0x8000 != 0 => (w & 0x7FFF) as u64 * 1024,
                    Some(w) => w as u64 * 1024 * 1024,
                };
                let speed = |off: usize, ext: usize| match s.word(off) {
                    Some(0) | None => None,
                    Some(0xFFFF) => s.dword(ext).filter(|&v| v > 0),
                    Some(v) => Some(v as u32),
                };
                // the speed it runs at, else the speed it is rated for
                let speed_mts = speed(0x20, 0x58).or_else(|| speed(0x15, 0x54));
                t.memory.push(MemoryModule {
                    locator: s.string(0x10).unwrap_or_default(),
                    size,
                    kind: s.byte(0x12).and_then(memory_kind).map(str::to_string),
                    speed_mts,
                    maker: s.string(0x17),
                });
            }
            _ => {}
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A structure: formatted bytes (header included) and its strings.
    fn st(kind: u8, mut body: Vec<u8>, strings: &[&str]) -> Vec<u8> {
        let mut v = vec![kind, (body.len() + 4) as u8, 0, 0];
        v.append(&mut body);
        if strings.is_empty() {
            v.extend([0, 0]);
        } else {
            for s in strings {
                v.extend(s.as_bytes());
                v.push(0);
            }
            v.push(0);
        }
        v
    }

    fn raw(structs: &[Vec<u8>]) -> Vec<u8> {
        let table: Vec<u8> = structs.concat();
        let mut r = vec![0, 3, 4, 0];
        r.extend((table.len() as u32).to_le_bytes());
        r.extend(table);
        r
    }

    #[test]
    fn board_bios_and_memory() {
        let mut bios = vec![0u8; 0x14];
        bios[0] = 1; // vendor = string 1 (offsets below are minus the 4-byte header)
        bios[1] = 2; // version
        bios[4] = 3; // date
        let mut sys = vec![0u8; 0x15];
        sys[0] = 1;
        sys[1] = 2;
        let mut board = vec![0u8; 0x0B];
        board[0] = 1;
        board[1] = 2;
        let mut mem = vec![0u8; 0x5C - 4];
        mem[0x0C - 4..0x0E - 4].copy_from_slice(&8192u16.to_le_bytes()); // 8 GB
        mem[0x10 - 4] = 1; // locator
        mem[0x12 - 4] = 0x1A; // DDR4
        mem[0x15 - 4..0x17 - 4].copy_from_slice(&3200u16.to_le_bytes());
        mem[0x17 - 4] = 2; // maker
        mem[0x20 - 4..0x22 - 4].copy_from_slice(&2666u16.to_le_bytes()); // configured
        let mut empty = vec![0u8; 0x28 - 4];
        empty[0x0C - 4..0x0E - 4].copy_from_slice(&0u16.to_le_bytes());
        let r = raw(&[
            st(0, bios, &["LENOVO", "GLCN44WW", "05/30/2023"]),
            st(1, sys, &["LENOVO", "82LM"]),
            st(2, board, &["LENOVO", "To Be Filled By O.E.M."]),
            st(17, mem, &["DIMM 0", "Samsung"]),
            st(17, empty, &["DIMM 1"]),
            st(127, vec![], &[]),
        ]);
        let t = parse(&r);
        assert_eq!(t.board.bios_vendor.as_deref(), Some("LENOVO"));
        assert_eq!(t.board.bios_version.as_deref(), Some("GLCN44WW"));
        assert_eq!(t.board.bios_date.as_deref(), Some("05/30/2023"));
        assert_eq!(t.board.system_product.as_deref(), Some("82LM"));
        assert_eq!(t.board.board_product, None, "placeholder text");
        assert_eq!(t.memory.len(), 1, "the empty slot is skipped");
        let m = &t.memory[0];
        assert_eq!((m.size, m.kind.as_deref(), m.speed_mts), (8 << 30, Some("DDR4"), Some(2666)));
        assert_eq!((m.locator.as_str(), m.maker.as_deref()), ("DIMM 0", Some("Samsung")));
    }

    #[test]
    fn broken_tables_dont_panic() {
        assert_eq!(parse(&[]), Tables::default());
        assert_eq!(parse(&[0, 3, 4, 0, 255, 255, 0, 0, 17, 2]), Tables::default());
        let mut r = raw(&[st(1, vec![1, 2, 0, 0], &["A"])]);
        r.truncate(r.len() - 2); // strings never end
        let _ = parse(&r);
    }
}
