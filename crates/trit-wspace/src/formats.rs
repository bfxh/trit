//! 模型/工件格式的**统一清单**：谁读得进、谁写得出的唯一事实来源。
//!
//! 这张表是给调用方（CLI `formats`、MCP 文档、外部脚本）看的，不是自动发现的：
//! 每条都对应本 crate 里一条真实编解码路径；`read` / `write` 为 false 的项就是
//! **明确没做**的部分（PyTorch `state_dict`、llama.cpp 侧的 `TYPE_TRIT`）。

use serde_json::{Value, json};

/// 一种容器/权重格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatInfo {
    /// 格式名（稳定标识）。
    pub name: &'static str,
    /// 常见扩展名（含点；空表示没有固定扩展名）。
    pub extensions: &'static [&'static str],
    /// 能否读入。
    pub read: bool,
    /// 能否写出。
    pub write: bool,
    /// 一句话边界说明。
    pub note: &'static str,
}

impl FormatInfo {
    /// → JSON。
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "extensions": self.extensions,
            "read": self.read,
            "write": self.write,
            "note": self.note,
        })
    }
}

/// 全部格式（顺序：源码格式 → 存储格式 → 未实现的桥）。
#[must_use]
pub fn registry() -> &'static [FormatInfo] {
    &[
        FormatInfo {
            name: "wsp1",
            extensions: &[".wsp"],
            read: true,
            write: true,
            note: "文本源码格式：逐 trit 可读、可 diff、带校验和",
        },
        FormatInfo {
            name: "wsp1g",
            extensions: &[".wsp1g"],
            read: true,
            write: true,
            note: "分组三值文本（wsp1 扩展，含每个元素的组号）",
        },
        FormatInfo {
            name: "wsp2",
            extensions: &[".wsp2"],
            read: true,
            write: true,
            note: "2-bit 二进制缓存/传输格式（含块索引与随机读）",
        },
        FormatInfo {
            name: "wsp2g",
            extensions: &[".wsp2g"],
            read: true,
            write: true,
            note: "分组三值二进制（WSP2 v3；组号 ceil(log2 组数) 位）",
        },
        FormatInfo {
            name: "wsp3",
            extensions: &[".wsp3"],
            read: true,
            write: true,
            note: "RLE 压缩流（不能按块随机读）",
        },
        FormatInfo {
            name: "wsp4",
            extensions: &[".wsp4"],
            read: true,
            write: true,
            note: "RLE + Huffman 压缩流（不能按块随机读）",
        },
        FormatInfo {
            name: "safetensors",
            extensions: &[".safetensors"],
            read: true,
            write: true,
            note: "f32/f16/bf16；单张量/多层/目录导入导出",
        },
        FormatInfo {
            name: "gguf",
            extensions: &[".gguf"],
            read: true,
            write: true,
            note: "三值 GGUF（TYPE_TRIT=100，自定义类型）",
        },
        FormatInfo {
            name: "pytorch-state-dict",
            extensions: &[".pt", ".pth", ".bin"],
            read: false,
            write: false,
            note: "未实现；只能先转成 safetensors",
        },
        FormatInfo {
            name: "llama.cpp-gguf",
            extensions: &[],
            read: false,
            write: false,
            note: "上游不认 TYPE_TRIT=100，未实测加载",
        },
    ]
}

/// 全部格式的 JSON 清单（`schema = trit-wspace.formats.v1`）。
#[must_use]
pub fn registry_json() -> Value {
    json!({
        "schema": "trit-wspace.formats.v1",
        "formats": registry().iter().map(FormatInfo::to_json).collect::<Vec<_>>(),
    })
}
