//! rust-analyzer 实时流守护进程：LSP over stdio 的最小客户端（零依赖、零进程创建）。
//!
//! 分层：
//! 1. **帧协议**（纯函数，可测）：[`encode_frame`]/[`read_frame`]——
//!    `Content-Length: N\r\n\r\n<body>`，与 LSP 规范一致；
//! 2. **会话**（泛型 IO）：[`LspSession`]——initialize 握手、didOpen/didSave
//!    通知、[`LspSession::pump`] 泵读诊断通知。对端可以是子进程 stdio、
//!    TCP 流或测试内存流——协议逻辑同一份代码路径；
//! 3. **落账**：pump 出的帧逐条喂 [`crate::lsp::LspBridge`]（URI→节点映射 +
//!    严重级→三值裁决），与 server 的 `/v1/lsp` 瞄准同一入账路径。
//!
//! # 进程启动留给部署层
//!
//! 内核不做进程创建（spawn 是应用/部署层的配置决定：PATH 里的
//! `rust-analyzer`、安装路径、版本管理都是部署策略）。集成方式（部署层
//! 参考实现，非内核代码）：
//!
//! ```text
//! let child = std::process::Command::new("rust-analyzer")   // 部署层自己校验 binary 来源
//!     .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
//! let mut session = LspSession::new(child.stdout.take().unwrap(), child.stdin.take().unwrap());
//! session.initialize(Some("file:///path/to/repo"))?;
//! session.did_open(uri, text, "rust")?;
//! let frames = session.pump(expected_frames)?;   // didSave 后按预期帧数泵
//! for f in &frames { bridge.process(f)?; }        // 与 /v1/lsp 同一入账路径
//! ```
//!
//! 阻塞语义（std 无就绪度轮询，如实声明）：[`LspSession::pump`] 阻塞读帧
//! 直到读满 `max_frames` 或对端 EOF——调用方在 didSave 后按预期帧数泵。

use std::io::{self, BufRead, BufReader, Read, Write};

/// 编码 LSP 帧：`Content-Length: N\r\n\r\n<body>`。
pub fn encode_frame(body: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 32);
    out.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
    out.extend_from_slice(body.as_bytes());
    out
}

/// 单条帧头行字节上限（与 trit-lsp framing 同口径）：超长头在分配前拒绝。
pub const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;
/// 单帧帧体字节上限：`Content-Length` 声明值超过它在分配前拒绝。
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// 帧体尺寸门（纯函数，不分配）：声明长度在硬顶内才放行。
fn frame_len_ok(len: usize) -> bool {
    len <= MAX_FRAME_BYTES
}

/// 读取一帧。`None` = 对端 EOF（干净结束）。
/// 头部逐行收（只认 `Content-Length`，其余忽略），空行结束头，随后读定长 body。
/// 帧头行与帧体都有硬顶，超限在分配前拒绝（对端可以是 TCP 流，不许用无界头/超长
/// `Content-Length` 撑爆内存——与 trit-lsp framing 同一纪律，审计 U-2 的同类落点）。
pub fn read_frame<R: BufRead>(r: &mut R) -> io::Result<Option<String>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut buf = Vec::new();
        let n =
            Read::take(&mut *r, (MAX_HEADER_LINE_BYTES + 1) as u64).read_until(b'\n', &mut buf)?;
        if n == 0 {
            return Ok(None); // EOF：干净结束
        }
        if buf.len() > MAX_HEADER_LINE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "帧头行超过上限（8 KiB）",
            ));
        }
        let line = String::from_utf8(buf).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("帧头不是 UTF-8：{e}"))
        })?;
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            content_length = Some(v.trim().parse::<usize>().map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("坏 Content-Length：{e}"),
                )
            })?);
        }
    }
    let len = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "帧缺少 Content-Length 头"))?;
    if !frame_len_ok(len) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Content-Length 超过上限（{MAX_FRAME_BYTES} 字节）：{len}"),
        ));
    }
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve_exact(len).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("帧体分配失败（{len} 字节）"),
        )
    })?;
    buf.resize(len, 0u8);
    r.read_exact(&mut buf)?;
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

/// JSON 字符串字面量转义——实现已归位 [`crate::json`]（内核），
/// 此处保留路径以兼容既有调用方。
pub use crate::json::json_escape;

/// 泛型 IO 的 LSP 会话：发通知/请求 + 泵读通知流。
/// `pending` 缓存请求应答途中混入的通知（不丢消息，下次 pump 先吐）。
pub struct LspSession<R: Read, W: Write> {
    reader: BufReader<R>,
    writer: W,
    next_id: u64,
    pending: Vec<String>,
}

impl<R: Read, W: Write> LspSession<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer,
            next_id: 1,
            pending: Vec::new(),
        }
    }

    /// 发一条通知（无 id，不期待应答）。
    pub fn notify(&mut self, method: &str, params_json: &str) -> io::Result<()> {
        let body = format!(
            "{{\"jsonrpc\":\"2.0\",\"method\":{},\"params\":{params_json}}}",
            json_escape(method)
        );
        self.writer.write_all(&encode_frame(&body))?;
        self.writer.flush()
    }

    /// 发一条请求并等待匹配 id 的应答；途中的通知缓存进 `pending`。
    /// 返回应答帧原文（调用方自行 `crate::json::parse` 取 result）。
    pub fn request(&mut self, method: &str, params_json: &str) -> io::Result<String> {
        let id = self.next_id;
        self.next_id += 1;
        let body = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":{},\"params\":{params_json}}}",
            json_escape(method)
        );
        self.writer.write_all(&encode_frame(&body))?;
        self.writer.flush()?;
        loop {
            let Some(frame) = read_frame(&mut self.reader)? else {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    format!("对端在等待 {method} 应答时关闭"),
                ));
            };
            let parsed = crate::json::parse(&frame).map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidData, format!("坏 JSON 帧：{e}"))
            })?;
            match parsed.get("id") {
                Some(v) => {
                    let matches = match v {
                        crate::json::Json::Num(n) => *n == id as f64,
                        _ => false,
                    };
                    if matches {
                        return Ok(frame);
                    }
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("应答 id 不匹配（期待 {id}）：{frame}"),
                    ));
                }
                None => self.pending.push(frame),
            }
        }
    }

    /// 泵读通知：先吐 `pending`（请求途中缓存的通知），再按需阻塞读新帧。
    /// 返回每帧 JSON 原文（≤ max_frames 条；EOF 提前停）。
    pub fn pump(&mut self, max_frames: usize) -> io::Result<Vec<String>> {
        let mut out: Vec<String> = std::mem::take(&mut self.pending);
        while out.len() < max_frames {
            match read_frame(&mut self.reader)? {
                Some(frame) => out.push(frame),
                None => break,
            }
        }
        Ok(out)
    }

    /// initialize 握手：initialize 请求（等应答）+ initialized 通知。
    pub fn initialize(&mut self, root_uri: Option<&str>) -> io::Result<String> {
        let params = match root_uri {
            Some(u) => format!("{{\"capabilities\":{{}},\"rootUri\":{}}}", json_escape(u)),
            None => "{\"capabilities\":{}}".to_string(),
        };
        let resp = self.request("initialize", &params)?;
        self.notify("initialized", "{}")?;
        Ok(resp)
    }

    /// textDocument/didOpen：打开文档（服务器随后推送首诊）。
    pub fn did_open(&mut self, uri: &str, text: &str, language_id: &str) -> io::Result<()> {
        let params = format!(
            "{{\"textDocument\":{{\"uri\":{},\"languageId\":{},\"version\":1,\"text\":{}}}}}",
            json_escape(uri),
            json_escape(language_id),
            json_escape(text)
        );
        self.notify("textDocument/didOpen", &params)
    }

    /// textDocument/didSave：保存（check on save 触发再诊）。
    pub fn did_save(&mut self, uri: &str) -> io::Result<()> {
        let params = format!("{{\"textDocument\":{{\"uri\":{}}}}}", json_escape(uri));
        self.notify("textDocument/didSave", &params)
    }
}

#[cfg(test)]
mod tests;
