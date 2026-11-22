//! bits 单测。

    use super::*;

    #[test]
    fn detects_widths_units_and_multi_value() {
        let info = analyze("使用 int8 与 4-bit 权重，对比 16 bit 激活；内存 512 MB，吞吐 2 GB/s。");
        assert!(info.widths.contains_key("8bit"));
        assert!(info.widths.contains_key("4bit"));
        assert!(info.widths.contains_key("16bit"));
        assert!(info.units.contains_key("mb"));
        assert!(info.units.contains_key("gb"));
        assert!(info.multi_valued);
    }

    #[test]
    fn bit_ops_and_labels() {
        let info = analyze("x = a << 2 | b & mask;");
        assert!(info.bit_ops);
        let l = labels("8-bit 量化与 ternary 权重");
        assert!(l.contains(&"8bit"));
        assert!(l.contains(&"multi-valued"));
        assert!(l.contains(&"trit"));
        assert!(labels("普通文本").is_empty());
    }
