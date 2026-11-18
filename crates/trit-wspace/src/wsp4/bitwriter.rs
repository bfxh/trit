//! WSP4 的 MSB-first 位流写入器（从 `wsp4.rs` 拆出，模块预算"要腾就拆"）。

/// 把 `(code, len)` 序列按 MSB-first 写进字节流；最后不足 8 bit 补零。
pub(super) struct BitWriter {
    out: Vec<u8>,
    current: u8,
    bits: u8,
}

impl BitWriter {
    pub(super) fn new() -> Self {
        Self {
            out: Vec::new(),
            current: 0,
            bits: 0,
        }
    }

    pub(super) fn write(&mut self, value: u32, len: u8) {
        for index in (0..len).rev() {
            let bit = (value >> index) & 1;
            self.current |= u8::try_from(bit).expect("bit") << (7 - self.bits);
            self.bits += 1;
            if self.bits == 8 {
                self.out.push(self.current);
                self.current = 0;
                self.bits = 0;
            }
        }
    }

    pub(super) fn finish(mut self, out: &mut Vec<u8>) {
        if self.bits > 0 {
            self.out.push(self.current);
        }
        out.extend_from_slice(&self.out);
    }
}
