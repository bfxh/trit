//! 连接监听循环 + 资源上限（审计 M1）：每连接设读写超时、并发数设上限，
//! 防 slowloris（半开连接永久占线程）与连接耗尽（每连接一线程却不设上限）。

use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// 单连接读写超时：半开连接（只发半个头）到点即被关，不再永久占一个线程。
const REQUEST_WAIT: Duration = Duration::from_secs(15);
/// 并发连接上限：超出的连接立即关闭，线程数有界。
const MAX_CONN: usize = 256;

/// 尝试占一个并发位：到 `max` 则回滚计数并拒绝（返回 false）。
fn admit(live: &AtomicUsize, max: usize) -> bool {
    if live.fetch_add(1, Ordering::SeqCst) >= max {
        live.fetch_sub(1, Ordering::SeqCst);
        false
    } else {
        true
    }
}

/// 起监听循环：每连接一线程，先判并发上限、设读写超时，再交给 handler。
pub(crate) fn serve<H>(listener: TcpListener, handler: H)
where
    H: Fn(TcpStream) + Clone + Send + Sync + 'static,
{
    let live = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            if !admit(&live, MAX_CONN) {
                continue; // 超上限：drop 即关闭，不占线程
            }
            let _ = stream.set_read_timeout(Some(REQUEST_WAIT));
            let _ = stream.set_write_timeout(Some(REQUEST_WAIT));
            let live = Arc::clone(&live);
            let handler = handler.clone();
            std::thread::spawn(move || {
                handler(stream);
                live.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admit_caps_concurrent_connections() {
        let live = AtomicUsize::new(0);
        assert!(admit(&live, 2));
        assert!(admit(&live, 2));
        assert!(!admit(&live, 2), "第三个连接应被拒（到并发上限）");
        assert_eq!(live.load(Ordering::SeqCst), 2, "被拒的要回滚计数");
        // 释放一个后可再收一个
        live.fetch_sub(1, Ordering::SeqCst);
        assert!(admit(&live, 2));
    }
}
