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

/// 读取一帧。`None` = 对端 EOF（干净结束）。
/// 头部逐行收（只认 `Content-Length`，其余忽略），空行结束头，随后读定长 body。
pub fn read_frame<R: BufRead>(r: &mut R) -> io::Result<Option<String>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line)?;
        if n == 0 {
            return Ok(None); // EOF：干净结束
        }
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
    let mut buf = vec![0u8; len];
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
mod tests {
    use super::*;
    use crate::lsp::LspBridge;
    use crate::{Journal, NodeId, Trit};
    use std::io::Cursor;

    fn frame(body: &str) -> Vec<u8> {
        encode_frame(body)
    }

    #[test]
    fn frame_roundtrip_and_eof() {
        let mut wire: Vec<u8> = Vec::new();
        wire.extend_from_slice(&frame("{\"a\":1}"));
        wire.extend_from_slice(&frame("second"));
        let mut r = Cursor::new(wire);
        assert_eq!(read_frame(&mut r).unwrap(), Some("{\"a\":1}".to_string()));
        assert_eq!(read_frame(&mut r).unwrap(), Some("second".to_string()));
        assert_eq!(read_frame(&mut r).unwrap(), None, "EOF 干净结束");
    }

    #[test]
    fn bad_content_length_is_rejected() {
        let mut r = Cursor::new(b"Content-Length: abc\r\n\r\n{}".to_vec());
        assert!(read_frame(&mut r).is_err());
        let mut r = Cursor::new(b"Content-Type: x\r\n\r\n{}".to_vec());
        assert!(read_frame(&mut r).is_err(), "缺 Content-Length 应拒绝");
    }

    #[test]
    fn json_escape_covers_specials() {
        assert_eq!(json_escape("a\"b\\c\nd\te"), "\"a\\\"b\\\\c\\nd\\te\"");
        assert_eq!(json_escape("\u{1}"), "\"\\u0001\"");
    }

    /// 假 LSP 服务器：预录 initialize 应答 + 两条诊断通知。
    fn fake_server_wire() -> Vec<u8> {
        let mut wire = Vec::new();
        wire.extend_from_slice(&frame(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"capabilities\":{\"textDocumentSync\":1}}}",
        ));
        wire.extend_from_slice(&frame(
            "{\"jsonrpc\":\"2.0\",\"method\":\"textDocument/publishDiagnostics\",\"params\":{\"uri\":\"file:///w/src/lib.rs\",\"diagnostics\":[{\"range\":{},\"severity\":1,\"message\":\"borrow checker 报错\"}]}}",
        ));
        wire.extend_from_slice(&frame(
            "{\"jsonrpc\":\"2.0\",\"method\":\"textDocument/publishDiagnostics\",\"params\":{\"uri\":\"file:///w/src/other.rs\",\"diagnostics\":[]}}",
        ));
        wire
    }

    #[test]
    fn session_full_flow_initialize_open_pump_and_ledger() {
        let wire = fake_server_wire();

        let mut session = LspSession::new(Cursor::new(wire.clone()), Vec::new());
        let resp = session.initialize(None).unwrap();
        assert!(resp.contains("capabilities"), "应答应含 capabilities");
        session
            .did_open("file:///w/src/lib.rs", "fn main() {}", "rust")
            .unwrap();
        let frames = session.pump(10).unwrap();
        assert_eq!(frames.len(), 2, "两条诊断通知都应泵到");
        assert!(frames[0].contains("publishDiagnostics"));

        // 全链路落账：pump 帧 → LspBridge → journal
        let p = std::env::temp_dir().join(format!("trit-daemon-{}.journal", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut j = Journal::create_as(&p, "daemon", 0).unwrap();
        let lib = j
            .spawn(NodeId(0), crate::NodeKind::Delta, "lib.rs")
            .unwrap();
        j.record(lib, Trit::Unknown, None, "打开待检").unwrap();
        {
            let mut bridge = LspBridge {
                journal: &mut j,
                uri_map: Default::default(),
            };
            bridge.map_uri("file:///w/src/lib.rs", lib);
            for f in &frames {
                bridge.process(f).unwrap();
            }
        }
        assert_eq!(j.dag().node(lib).verdict, Trit::Vulnerable, "E1 → -1");
        drop(j.into_dag().unwrap());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn request_buffers_intermediate_notifications() {
        let mut wire = Vec::new();
        wire.extend_from_slice(&frame(
            "{\"jsonrpc\":\"2.0\",\"method\":\"window/logMessage\",\"params\":{\"message\":\"hi\"}}",
        ));
        wire.extend_from_slice(&frame(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}",
        ));
        let mut session = LspSession::new(Cursor::new(wire), Vec::new());
        let resp = session.request("foo/bar", "{}").unwrap();
        assert!(resp.contains("\"ok\":true"));
        let drained = session.pump(10).unwrap();
        assert_eq!(drained.len(), 1, "中途通知应缓存并在 pump 时吐出");
        assert!(drained[0].contains("logMessage"));
    }

    #[test]
    fn eof_during_request_is_broken_pipe() {
        let mut session = LspSession::new(Cursor::new(Vec::new()), Vec::new());
        assert!(session.request("foo/bar", "{}").is_err());
    }
}
