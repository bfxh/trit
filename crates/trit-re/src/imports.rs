//! 导入面 → 能力桶。
//!
//! 逆向里"导入了什么"比"叫什么名字"可靠：改名/加壳不改变导入的 API 语义。
//! 这里只做**名字级归类**，不做行为推断；每个桶命中哪些函数会原样进证据。
//!
//! 归类是保守的：表里没有的 API 不猜。命中表里的函数也不等于恶意——
//! `LoadLibraryA` 常见于正常程序，结论由三值判定层结合其他证据给出。

use std::collections::BTreeMap;

/// 一个能力桶。
#[derive(Debug, Clone, Copy)]
pub struct Bucket {
    /// 稳定 id（进台账）。
    pub id: &'static str,
    /// 人读标题。
    pub title: &'static str,
    /// 归入本桶的函数名（区分大小写不敏感匹配）。
    pub apis: &'static [&'static str],
}

/// 全部能力桶。
pub const BUCKETS: &[Bucket] = &[
    Bucket {
        id: "imports.inj.alloc",
        title: "在目标进程里申请/改权限内存",
        apis: &[
            "VirtualAllocEx",
            "NtAllocateVirtualMemory",
            "VirtualProtectEx",
            "NtProtectVirtualMemory",
            "MapViewOfFile",
        ],
    },
    Bucket {
        id: "imports.inj.write",
        title: "写目标进程内存",
        apis: &["WriteProcessMemory", "NtWriteVirtualMemory"],
    },
    Bucket {
        id: "imports.inj.exec",
        title: "在目标进程里起执行",
        apis: &[
            "CreateRemoteThread",
            "CreateRemoteThreadEx",
            "NtCreateThreadEx",
            "RtlCreateUserThread",
            "QueueUserAPC",
            "SetThreadContext",
            "ResumeThread",
        ],
    },
    Bucket {
        id: "imports.loader",
        title: "运行期动态装载",
        apis: &[
            "LoadLibraryA",
            "LoadLibraryW",
            "LoadLibraryExA",
            "LoadLibraryExW",
            "GetProcAddress",
            "LdrLoadDll",
            "LdrGetProcedureAddress",
            "dlopen",
            "dlsym",
        ],
    },
    Bucket {
        id: "imports.network",
        title: "网络收发",
        apis: &[
            "WSAStartup",
            "WSASocketA",
            "WSASocketW",
            "socket",
            "connect",
            "send",
            "recv",
            "InternetOpenA",
            "InternetOpenW",
            "InternetConnectA",
            "InternetConnectW",
            "HttpOpenRequestA",
            "HttpOpenRequestW",
            "HttpSendRequestA",
            "HttpSendRequestW",
            "WinHttpOpen",
            "WinHttpConnect",
            "URLDownloadToFileA",
            "URLDownloadToFileW",
        ],
    },
    Bucket {
        id: "imports.credential",
        title: "凭据/令牌访问",
        apis: &[
            "CryptUnprotectData",
            "MiniDumpWriteDump",
            "LsaEnumerateLogonSessions",
            "SamQueryInformationUser",
            "OpenProcessToken",
            "ImpersonateLoggedOnUser",
            "LogonUserA",
            "LogonUserW",
            "CredReadA",
            "CredReadW",
        ],
    },
    Bucket {
        id: "imports.persistence",
        title: "开机/登录持久化",
        apis: &[
            "RegSetValueExA",
            "RegSetValueExW",
            "RegCreateKeyExA",
            "RegCreateKeyExW",
            "CreateServiceA",
            "CreateServiceW",
            "SetWindowsHookExA",
            "SetWindowsHookExW",
        ],
    },
    Bucket {
        id: "imports.anti_debug",
        title: "反调试/时间检测",
        apis: &[
            "IsDebuggerPresent",
            "CheckRemoteDebuggerPresent",
            "NtQueryInformationProcess",
            "OutputDebugStringA",
            "OutputDebugStringW",
            "ptrace",
        ],
    },
    Bucket {
        id: "imports.crypto",
        title: "加密原语",
        apis: &[
            "CryptEncrypt",
            "CryptDecrypt",
            "CryptAcquireContextA",
            "CryptAcquireContextW",
            "BCryptEncrypt",
            "BCryptDecrypt",
            "CryptGenKey",
        ],
    },
];

/// 从 `dll!Api@version` 里取出函数名（去 dll 与 ELF 版本后缀）。
pub fn api_name(import: &str) -> &str {
    let after_dll = import.rsplit_once('!').map_or(import, |(_, api)| api);
    match after_dll.split_once('@') {
        Some((name, _version)) => name,
        None => after_dll,
    }
}

/// 归类：桶 id → 命中的原始导入串（保持原有 `dll!Api` 形式）。
///
/// 只返回非空桶；调用方据此给每个桶写一条三值事实。
pub fn classify(imports: &[String]) -> BTreeMap<&'static str, Vec<String>> {
    let mut hits: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for import in imports {
        let api = api_name(import);
        for bucket in BUCKETS {
            if bucket
                .apis
                .iter()
                .any(|known| known.eq_ignore_ascii_case(api))
            {
                hits.entry(bucket.id).or_default().push(import.clone());
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::{api_name, classify};

    #[test]
    fn strips_dll_and_version() {
        assert_eq!(
            api_name("KERNEL32.dll!CreateRemoteThread"),
            "CreateRemoteThread"
        );
        assert_eq!(api_name("socket@GLIBC_2.2.5"), "socket");
        assert_eq!(api_name("plain"), "plain");
    }

    #[test]
    fn classifies_case_insensitively_and_keeps_originals() {
        let imports = vec![
            "KERNEL32.dll!virtualallocex".to_string(),
            "kernel32.dll!WriteProcessMemory".to_string(),
            "libc.so.6!connect".to_string(),
            "KERNEL32.dll!Sleep".to_string(),
        ];
        let hits = classify(&imports);
        assert_eq!(
            hits["imports.inj.alloc"],
            vec!["KERNEL32.dll!virtualallocex"]
        );
        assert_eq!(
            hits["imports.inj.write"],
            vec!["kernel32.dll!WriteProcessMemory"]
        );
        assert_eq!(hits["imports.network"], vec!["libc.so.6!connect"]);
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn unknown_apis_stay_unclassified() {
        assert!(classify(&["KERNEL32.dll!ExitProcess".to_string()]).is_empty());
    }
}
