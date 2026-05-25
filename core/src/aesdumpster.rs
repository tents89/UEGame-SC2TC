// Thanks: 
// AESDumpster - GHFear
// yuhkix a- esdumpster-rs
use std::path::Path;

// ── KeyDumpster ──────────────────────────────────────────────────────────────


pub struct KeyDumpster {
    pub key_vector: Vec<String>,
    pub key_entropies: Vec<f64>,

    key_patterns: Vec<&'static str>,
    false_positives: Vec<&'static str>,
    key_dword_offsets: Vec<Vec<usize>>,
}

impl Default for KeyDumpster {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyDumpster {
    pub fn new() -> Self {
        Self {
            key_vector: Vec::new(),
            key_entropies: Vec::new(),
            key_patterns: vec![
                "C7 ? ? ? ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? ? ? ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? ?",
                "C7 ? ? ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? ?",
                "C7 ? ? ? ? ? ? C7 ? ? ? ? ? ? 48 ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? ?",
                "C7 ? ? ? ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? ? C7 ? ? ? ? ? C3",
            ],
            false_positives: vec![
                "FFD9FFD9FFD9FFD9FFD9FFD9FFD9FFD9FFD9FFD9FFD9FFD9FFD9FFD9FFD9FFD9",
                "67E6096A85AE67BB72F36E3C3AF54FA57F520E518C68059BABD9831F19CDE05B",
                "D89E05C107D57C3617DD703039590EF7310BC0FF11155868A78FF964A44FFABE",
                "9A99593F9A99593F0AD7633F52B8BE3FE17A543FCDCC4C3D4260E53BAE47A13F",
                "6F168073B9B21449D742241700068ADABC306FA9AA3831164DEE8DE34E0EFBB0",
                "0AD7633FCDCC4C3DCDCCCC3D52B8BE3F9A99593F9A99593FC9767E3FE17A543F",
                "168073C7B21449C7430C00064310BC304314AA3843184DEE431C4E0E83C4205B",
                "E6096AC7AE67BBC7430C3AF543107F5243148C684318ABD9431C19CD436C2000",
                "9E05C1C7D57C36C7430C39594310310B431411154318A78F431CA44F436C1C00",
                "9E05C1C7D57C36C7DD7030C7590EF7C70BC0FFC7155868C78FF964C7A44FFABE",
                "168073C7B21449C7422417C7068ADAC7306FA9C7383116C7EE8DE3C74E0EFBB0",
                "0AD7633FCDCC4C3D00C742143DC742183FC7421C3FC742203FC742247E3FC742",
                "0000803F0AD7A33E0AD7633F52B8BE3FE17A543FCDCC4C3D4260E53B54AE47A1",
                "0AD7A33E0AD7633F52B8BE3FE17A543FCDCC4C3D4260E53BAE47A13F38583934",
                "0000803F0AD7A33E0AD7633F52B8BE3FE17A543FCDCC4C3D4260E53B34AE47A1",
                "0000803F0000803F0AD7A33E0AD7633F52B8BE3FE17A543FCDCC4C3D2C4260E5",
                "0AD7633F52B8BE3FE17A543FCDCC4C3D4260E53BAE47A13F5839343C4CC9767E",
                "07D57C3617DD703039590EF7310BC0FF11155868A78FF964A44FFABE6C1C0000",
                "85AE67BB72F36E3C3AF54FA57F520E518C68059BABD9831F19CDE05B6C200000",
                "E6096AC7AE67BBC7F36E3CC7F54FA5C7520E51C768059BC7D9831FC719CDE05B",
                "0AD7A33E0AD7633F52B8BE3FE17A543FCDCC4C3D4260E53BAE47A13F3C583934",
                "E4D6E74FE4D667500044AC47926595380080DC43000A9B46000080BF000080BF",
                "D04C8F7D71ECC047D8A60970FBA31C9E9EC1250BBBF6459AC480947212E1DB8C",
            ],
            key_dword_offsets: vec![
                vec![3, 10, 17, 24, 35, 42, 49, 56],
                vec![2, 9, 16, 23, 30, 37, 44, 51],
                vec![3, 10, 21, 28, 35, 42, 49, 56],
                vec![51, 45, 38, 31, 24, 17, 10, 3],
            ],
        }
    }

    pub fn find_aes_keys(&mut self, buffer: &[u8]) -> bool {
        for (i, pattern) in self.key_patterns.iter().enumerate() {
            let matches = find_signature(buffer, pattern);
            let offsets = &self.key_dword_offsets[i];
            for base in matches {
                if let Some(hex) = self.concatenate_aes_type(buffer, base, offsets) {
                    self.key_vector.push(hex);
                }
            }
        }

        self.key_entropies = self.key_entropy_generator();

        // 只需確認確實有候選 key 與對應 entropy；後續 get_most_likely_key
        // 會自行掃描 entropies 並排除 false_positives。
        !self.key_vector.is_empty() && !self.key_entropies.is_empty()
    }

    pub fn get_most_likely_key(&self) -> Option<String> {
        let mut best_key_index: Option<usize> = None;
        let mut max_entropy = f64::NEG_INFINITY;

        for (i, key) in self.key_vector.iter().enumerate() {
            if let Some(&ent) = self.key_entropies.get(i) {
                if ent >= 3.3
                    && ent > max_entropy
                    && !self.false_positives.iter().any(|&fp| fp == key)
                {
                    max_entropy = ent;
                    best_key_index = Some(i);
                }
            }
        }

        best_key_index.and_then(|i| self.key_vector.get(i).cloned())
    }

    fn concatenate_aes_type(&self, buf: &[u8], base: usize, offsets: &[usize]) -> Option<String> {
        let mut out = String::with_capacity(64); // 8 dwords × 8 hex chars
        for &off in offsets {
            let idx = base.checked_add(off)?;
            if idx + 4 > buf.len() {
                return None;
            }
            out.push_str(&hex_str(&buf[idx..idx + 4]));
        }
        Some(out.to_uppercase())
    }

    fn key_entropy_generator(&self) -> Vec<f64> {
        self.key_vector.iter().map(|k| calc_entropy(k)).collect()
    }
}

// ── 私有輔助函式 ─────────────────────────────────────────────────────────────

fn hex_str(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn calc_entropy(s: &str) -> f64 {
    let mut freq = [0usize; 256];
    for &b in s.as_bytes() {
        freq[b as usize] += 1;
    }
    let len = s.len() as f64;
    freq.iter()
        .filter(|&&count| count > 0)
        .map(|&count| {
            let p = count as f64 / len;
            -p * p.log2()
        })
        .sum()
}

fn parse_signature(pattern: &str) -> Vec<Option<u8>> {
    pattern
        .split_whitespace()
        .map(|tok| {
            if tok.contains('?') {
                None
            } else {
                u8::from_str_radix(tok, 16).ok()
            }
        })
        .collect()
}

fn find_signature(buf: &[u8], pattern: &str) -> Vec<usize> {
    let sig = parse_signature(pattern);
    let sig_len = sig.len();
    if sig_len == 0 || buf.len() < sig_len {
        return Vec::new();
    }
    (0..=buf.len() - sig_len)
        .filter(|&i| {
            sig.iter()
                .enumerate()
                .all(|(j, maybe)| maybe.is_none() || *maybe == Some(buf[i + j]))
        })
        .collect()
}

// ── 核心導出函式 ──────────────────────────────────────────────────────────────
/// 給予 EXE 路徑，返回找到的 AES Key（不含 0x 前綴）
pub fn extract_aes_key(exe_path: &Path) -> Option<String> {
    let buffer = std::fs::read(exe_path).ok()?;
    let mut dumpster = KeyDumpster::new();
    if dumpster.find_aes_keys(&buffer) {
        dumpster.get_most_likely_key()
    } else {
        None
    }
}