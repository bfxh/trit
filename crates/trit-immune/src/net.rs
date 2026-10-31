//! 出网守卫：URL 校验（SSRF 防护）。
//!
//! 硬约束：服务端请求 URL 时仅允许 http/https；请求前校验 host，
//! 拒绝 localhost、环回、私有和保留地址；域名解析后**对每个解析出的 IP 复核**
//! （防 DNS 重绑定绕过）。本 crate 的同步传输不经 URL（字面 `IP:端口`），
//! 本守卫供一切未来出网调用（网站、炼化数据源）统一使用。

use std::io;
use std::net::{IpAddr, ToSocketAddrs};

/// 校验一个出网 URL；通过才允许发起请求。
pub fn check_url(url: &str) -> Result<(), String> {
    check_url_with(url, &|host_port| {
        host_port
            .to_socket_addrs()
            .map(|it| it.map(|s| s.ip()).collect())
    })
}

/// 可注入解析器的校验（测试确定性 / 自定义 DNS 策略）。
pub fn check_url_with(
    url: &str,
    resolve: &dyn Fn(&str) -> io::Result<Vec<IpAddr>>,
) -> Result<(), String> {
    let bad = |why: &str| Err::<(), String>(format!("URL 拒绝（{why}）：{url:?}"));
    let Some((scheme, rest)) = url.split_once("://") else {
        return bad("缺 scheme");
    };
    if scheme != "http" && scheme != "https" {
        return bad("仅允许 http/https");
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // 拆 host:port（容忍 IPv6 字面量 [..]:port）
    let host = match authority.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => h,
        _ => authority,
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return bad("空 host");
    }
    if host.eq_ignore_ascii_case("localhost") {
        return bad("localhost");
    }
    match host.parse::<IpAddr>() {
        Ok(ip) => {
            if is_forbidden_ip(ip) {
                return bad("环回/私有/保留地址");
            }
            Ok(())
        }
        Err(_) => {
            // 域名：解析后逐 IP 复核（防 DNS 重绑定到内网）
            let ips = resolve(&format!("{host}:0"))
                .map_err(|_| format!("URL 拒绝（域名解析失败）：{url:?}"))?;
            if ips.is_empty() {
                return bad("域名无解析结果");
            }
            if ips.iter().any(|ip| is_forbidden_ip(*ip)) {
                return bad("解析到环回/私有/保留地址");
            }
            Ok(())
        }
    }
}

/// 地址黑名单：环回 / 私网 / 链路本地 / 保留 / 组播 / 未指定 / v4 映射的上述地址。
pub fn is_forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 0                                   // 0.0.0.0/8 未指定
                || o[0] == 10                           // 10/8 私网
                || o[0] == 127                          // 127/8 环回
                || (o[0] == 100 && (64..=127).contains(&o[1])) // 100.64/10 CGNAT
                || (o[0] == 169 && o[1] == 254)         // 169.254/16 链路本地
                || (o[0] == 172 && (16..=31).contains(&o[1])) // 172.16/12 私网
                || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0.0/24 保留
                || (o[0] == 192 && o[1] == 168)         // 192.168/16 私网
                || (o[0] == 198 && o[1] == 51 && o[2] == 100) // TEST-NET-2
                || (o[0] == 203 && o[1] == 0 && o[2] == 113) // TEST-NET-3
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19)) // 198.18/15 基准
                || o[0] >= 224                          // 224/4 组播 + 240/4 保留 + 广播
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || (s[0] & 0xfe00) == 0xfc00            // fc00::/7 ULA
                || (s[0] & 0xffc0) == 0xfe80            // fe80::/10 链路本地
                || (s[0] & 0xff00) == 0xff00            // ff00::/8 组播
                || (s[0] == 0
                    && s[1] == 0
                    && s[2] == 0
                    && s[3] == 0
                    && s[4] == 0
                    && s[5] == 0xffff
                    && is_forbidden_ip(IpAddr::V4(std::net::Ipv4Addr::new(
                        (s[6] >> 8) as u8,
                        s[6] as u8,
                        (s[7] >> 8) as u8,
                        s[7] as u8,
                    )))) // ::ffff:0:0/96 v4 映射
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve_ok(_: &str) -> io::Result<Vec<IpAddr>> {
        Ok(vec![IpAddr::from([93, 184, 216, 34])])
    }

    #[test]
    fn forbidden_urls_are_rejected() {
        for url in [
            "http://127.0.0.1:8080/x",
            "http://localhost/api",
            "http://LOCALHOST:80/x",
            "http://[::1]/x",
            "http://10.1.2.3/x",
            "http://172.16.0.9/x",
            "http://192.168.1.1/x",
            "http://169.254.1.1/x",
            "http://0.0.0.0/x",
            "http://100.64.0.1/x",
            "http://224.0.0.1/x",
            "http://[fe80::1]/x",
            "http://[fc00::1]/x",
            "http://[::ffff:127.0.0.1]/x",
        ] {
            assert!(check_url_with(url, &resolve_ok).is_err(), "{url} 应被拒绝");
        }
        // 非 http/https 与缺 scheme
        assert!(check_url_with("ftp://example.com", &resolve_ok).is_err());
        assert!(check_url_with("example.com/x", &resolve_ok).is_err());
        // 域名解析到内网（DNS 重绑定）也要拦
        assert!(check_url_with("http://evil.example.com/x", &|_| Ok(vec![IpAddr::from([10, 0, 0, 1])])).is_err());
        // 解析失败
        assert!(check_url_with("http://no-such-host.invalid/x", &|_| Err(io::Error::new(io::ErrorKind::Other, "dns"))).is_err());
    }

    #[test]
    fn public_urls_pass() {
        assert!(check_url_with("https://example.com/x", &resolve_ok).is_ok());
        assert!(check_url_with("http://example.com:8080/a?b=c", &resolve_ok).is_ok());
        // 非保留公网 IP 字面量
        assert!(check_url_with("http://93.184.216.34/", &resolve_ok).is_ok());
    }
}
