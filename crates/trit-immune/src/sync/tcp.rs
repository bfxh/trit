//! 同步帧的 TCP 传输（从 src/sync.rs 拆出，控制单文件行数预算）：
//! 8 字节大端长度 + UTF-8 帧（上限 64 MiB），单次 accept 的 pull/merge。

use super::*;
const MAX_FRAME: u64 = 64 * 1024 * 1024;

pub fn write_frame(stream: &mut TcpStream, text: &str) -> io::Result<()> {
    let bytes = text.as_bytes();
    stream.write_all(&(bytes.len() as u64).to_be_bytes())?;
    stream.write_all(bytes)
}

pub fn read_frame(stream: &mut TcpStream) -> io::Result<String> {
    let mut buf = [0u8; 8];
    stream.read_exact(&mut buf)?;
    let len = u64::from_be_bytes(buf);
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "帧超过 64 MiB 上限",
        ));
    }
    let mut body = vec![0u8; len as usize];
    stream.read_exact(&mut body)?;
    String::from_utf8(body).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "帧非 UTF-8"))
}

/// 客户端（无认证）。
pub fn pull_merge(addr: SocketAddr, local: &mut Journal) -> io::Result<usize> {
    pull_merge_with(addr, local, None)
}

/// 客户端（可选 PSK 帧认证：HMAC-SHA256，见 [`crate::auth`]）。
pub fn pull_merge_with(
    addr: SocketAddr,
    local: &mut Journal,
    auth_key: Option<&[u8]>,
) -> io::Result<usize> {
    let mine = parse_log_file(local.path())?;
    let w = local.clock().writer();
    let mut stream = TcpStream::connect(addr)?;
    let frame = encode_frame(w, &mine);
    let frame = match auth_key {
        Some(k) => {
            sign_frame(&frame, k).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
        }
        None => frame,
    };
    write_frame(&mut stream, &frame)?;
    let resp = read_frame(&mut stream)?;
    let canonical = match auth_key {
        Some(k) => verify_frame(&resp, k),
        None => parse_frame(&resp),
    }
    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let n = canonical.stamped.len();
    local.adopt(canonical.stamped)?;
    Ok(n)
}

/// 服务端（无认证）。
pub fn serve_merge(listener: TcpListener, local: &mut Journal) -> io::Result<MergeOutcome> {
    serve_merge_with(listener, local, None)
}

/// 服务端（可选 PSK 帧认证：HMAC-SHA256，见 [`crate::auth`]）。
pub fn serve_merge_with(
    listener: TcpListener,
    local: &mut Journal,
    auth_key: Option<&[u8]>,
) -> io::Result<MergeOutcome> {
    let (mut stream, _) = listener.accept()?;
    // 单次 merge 的读/写限时：对端半开不能把这次合并永久卡住（M1）。
    stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(30)))?;
    let frame = read_frame(&mut stream)?;
    let remote = match auth_key {
        Some(k) => verify_frame(&frame, k),
        None => parse_frame(&frame),
    }
    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mine = parse_log_file(local.path())?;
    let w = local.clock().writer();
    let out = merge_replicas(vec![(w, mine), (remote.writer, remote.stamped)]);
    let reply = encode_frame(w, &out.canonical);
    let reply = match auth_key {
        Some(k) => {
            sign_frame(&reply, k).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
        }
        None => reply,
    };
    write_frame(&mut stream, &reply)?;
    local.adopt(out.canonical.clone())?;
    Ok(out)
}
