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

/// 动态解析链的字符串证据：数据段里出现「敏感 API 名 + NUL」字面串，
/// 且该 API **不在导入表**——导入表可见的 API 不算"藏"。只搜 ASCII、
/// 名字 ≥6 字符（短名误报率高）。调用方给非可执行节的字节切片
///（可执行节里的栈串形态是下一层）。词汇表 = 注入/凭据家族的全部
/// 桶内 API 名（随 BUCKETS 自动同步）；load 家族不入表（良性串太多）。
pub fn hidden_api_strings(
    slices: &[&[u8]],
    imported_apis: &std::collections::BTreeSet<String>,
) -> Vec<&'static str> {
    let vocab: Vec<&'static str> = BUCKETS
        .iter()
        .filter(|b| b.id.starts_with("imports.inj.") || b.id == "imports.credential")
        .flat_map(|b| b.apis.iter().copied())
        .collect();
    let mut found = std::collections::BTreeSet::new();
    for name in vocab {
        if name.len() < 6 || imported_apis.contains(name) {
            continue;
        }
        let mut needle = name.as_bytes().to_vec();
        needle.push(0);
        for slice in slices {
            if slice.windows(needle.len()).any(|w| w == needle.as_slice()) {
                found.insert(name);
                break;
            }
        }
    }
    found.into_iter().collect()
}

/// 动态解析器是否在导入表（链的前半段证据）：PE = `GetProcAddress`，
/// ELF = `dlsym`（调用方给解析器名单；`api_name` 已剥版本后缀）。
pub fn resolver_imported(imports: &[String], resolvers: &[&str]) -> bool {
    imports.iter().any(|i| {
        let api = api_name(i);
        resolvers.iter().any(|r| r.eq_ignore_ascii_case(api))
    })
}

#[cfg(test)]
mod tests {
    use super::{api_name, classify, hidden_api_strings, resolver_imported};

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

    #[test]
    fn hidden_api_strings_skips_imported_and_short() {
        use std::collections::BTreeSet;
        let mut imported = BTreeSet::new();
        imported.insert("VirtualAllocEx".to_string()); // 已导入 = 可见，不算"藏"
        let slice = b"padding\x00WriteProcessMemory\x00tail\x00VirtualAllocEx\x00" as &[u8];
        let found = hidden_api_strings(&[slice], &imported);
        assert_eq!(found, vec!["WriteProcessMemory"], "已导入的被排除");
        // 短名（<6 字符）不入词汇表：误报率高
        let short = b"x\x00sed\x00tail\x00" as &[u8];
        assert!(hidden_api_strings(&[short], &BTreeSet::new()).is_empty());
    }

    #[test]
    fn resolver_imported_matches_case_insensitively() {
        let imports = vec![
            "KERNEL32.dll!GetModuleHandleA".to_string(),
            "KERNEL32.dll!getprocaddress".to_string(),
        ];
        assert!(resolver_imported(&imports, &["GetProcAddress"]));
        assert!(resolver_imported(&imports, &["GetModuleHandleA"]));
        assert!(!resolver_imported(&imports, &["dlsym"]));
    }
}
