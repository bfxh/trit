//! 导入表解析与能力位分类。
//!
//! 从 `pe.rs` 拆出，守住单文件 800 行预算；语义与拆前一致。

use crate::reader::Reader;

use super::{cap, dir, PeInfo, DIR_IMPORT, MAX_IMPORT_DESCRIPTORS, MAX_THUNKS};

/// Import table summary. Names are borrowed from the file buffer, never copied.
#[derive(Clone, Default)]
pub struct Imports {
    /// Number of DLLs listed.
    pub dll_count: u32,
    /// Number of named functions listed.
    pub function_count: u32,
    /// Number of ordinal-only entries.
    pub ordinal_count: u32,
    /// Capability bits, see [`cap`].
    pub caps: u16,
    /// True when the whole table was walked without hitting a limit or a bad RVA.
    pub complete: bool,
    /// True when an import directory was declared at all.
    pub present: bool,
}

impl Imports {
    /// Tests a capability bit.
    #[must_use]
    pub fn has(&self, bit: u16) -> bool {
        self.caps & bit != 0
    }
}

pub(super) fn parse_imports(info: &PeInfo<'_>) -> Imports {
    let mut out = Imports::default();
    let d = dir(info, DIR_IMPORT);
    if d.address == 0 || d.size == 0 {
        out.complete = true;
        return out;
    }
    out.present = true;
    let mut complete = true;
    let step = 20usize;
    let Some(mut at) = info.rva_to_offset(d.address) else {
        out.complete = false;
        return out;
    };

    for _ in 0..MAX_IMPORT_DESCRIPTORS {
        let r = Reader::le(info.bytes);
        let (Some(oft), Some(name_rva), Some(ft)) = (r.u32(at), r.u32(at + 12), r.u32(at + 16))
        else {
            complete = false;
            break;
        };
        // All-zero descriptor terminates the array.
        if oft == 0 && name_rva == 0 && ft == 0 {
            break;
        }
        out.dll_count += 1;

        let thunks_at = if oft != 0 { oft } else { ft };
        match info.rva_to_offset(thunks_at) {
            Some(base) => {
                let width = if info.is_64 { 8 } else { 4 };
                for i in 0..MAX_THUNKS {
                    let slot = base + i * width;
                    let Some(entry) = Reader::le(info.bytes).uint(slot, info.is_64) else {
                        complete = false;
                        break;
                    };
                    if entry == 0 {
                        break;
                    }
                    let ordinal_flag = if info.is_64 { 1u64 << 63 } else { 1u64 << 31 };
                    if entry & ordinal_flag != 0 {
                        out.ordinal_count += 1;
                        continue;
                    }
                    let Some(name_off) = info.rva_to_offset((entry & 0x7FFF_FFFF) as u32) else {
                        complete = false;
                        continue;
                    };
                    // Skip the two-byte hint.
                    let mut name = &info.bytes[..0];
                    if let Some((bytes, terminated)) =
                        Reader::le(info.bytes).cstr(name_off + 2, 512)
                    {
                        name = bytes;
                        if !terminated {
                            complete = false;
                        }
                    } else {
                        complete = false;
                    }
                    out.function_count += 1;
                    out.caps |= classify_import(name);
                }
            }
            None => complete = false,
        }

        at += step;
        if at >= info.bytes.len() {
            complete = false;
            break;
        }
    }

    if out.dll_count as usize >= MAX_IMPORT_DESCRIPTORS {
        complete = false;
    }
    out.complete = complete;
    out
}

/// Maps an import name to capability bits.
///
/// Matching is case-insensitive on the full name. The list is intentionally
/// conservative: an API that appears in most legitimate programs (`RegSetValueEx`,
/// `CreateProcessA`) is not here, because a capability bit that fires everywhere
/// carries no information.
#[must_use]
pub fn classify_import(name: &[u8]) -> u16 {
    if name.is_empty() || name.len() > 64 {
        return 0;
    }
    let mut bits = 0u16;
    for rule in IMPORT_RULES {
        if eq_ignore_case(name, rule.0.as_bytes()) {
            bits |= rule.1;
        }
    }
    bits
}

pub(super) fn eq_ignore_case(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

/// `(name, capability bits)`.
type ImportRule = (&'static str, u16);

const IMPORT_RULES: &[ImportRule] = &[
    // Loader: module loading without a static import entry.
    ("LoadLibraryA", cap::LOADER),
    ("LoadLibraryW", cap::LOADER),
    ("LoadLibraryExA", cap::LOADER),
    ("LoadLibraryExW", cap::LOADER),
    ("LdrLoadDll", cap::LOADER),
    ("GetProcAddress", cap::DYNAMIC_RESOLVE),
    ("LdrGetProcedureAddress", cap::DYNAMIC_RESOLVE),
    // Network.
    ("InternetOpenA", cap::NETWORK),
    ("InternetOpenW", cap::NETWORK),
    ("InternetConnectA", cap::NETWORK),
    ("InternetConnectW", cap::NETWORK),
    ("InternetOpenUrlA", cap::NETWORK),
    ("InternetOpenUrlW", cap::NETWORK),
    ("InternetReadFile", cap::NETWORK),
    ("HttpOpenRequestA", cap::NETWORK),
    ("HttpSendRequestA", cap::NETWORK),
    ("HttpSendRequestW", cap::NETWORK),
    ("WinHttpOpen", cap::NETWORK),
    ("WinHttpConnect", cap::NETWORK),
    ("WinHttpSendRequest", cap::NETWORK),
    ("URLDownloadToFileA", cap::NETWORK),
    ("URLDownloadToFileW", cap::NETWORK),
    ("WSAStartup", cap::NETWORK),
    ("WSASocketA", cap::NETWORK),
    ("WSASocketW", cap::NETWORK),
    ("socket", cap::NETWORK),
    ("connect", cap::NETWORK),
    ("send", cap::NETWORK),
    ("sendto", cap::NETWORK),
    ("recv", cap::NETWORK),
    ("gethostbyname", cap::NETWORK),
    ("getaddrinfo", cap::NETWORK),
    // Credential and secret access.
    ("CredReadA", cap::CREDENTIAL),
    ("CredReadW", cap::CREDENTIAL),
    ("CredEnumerateA", cap::CREDENTIAL),
    ("CredEnumerateW", cap::CREDENTIAL),
    ("CryptUnprotectData", cap::CREDENTIAL),
    ("LsaEnumerateLogonSessions", cap::CREDENTIAL),
    ("LsaGetLogonSessionData", cap::CREDENTIAL),
    ("LsaOpenPolicy", cap::CREDENTIAL),
    ("SamQueryInformationUser", cap::CREDENTIAL),
    ("LogonUserA", cap::CREDENTIAL),
    ("LogonUserW", cap::CREDENTIAL),
    ("MiniDumpWriteDump", cap::CREDENTIAL),
    // Remote memory: split into three rails so only the trio escalates.
    //
    // The rails deliberately exclude the generic `Nt*` variants. `NtAllocateVirtualMemory`
    // and `NtWriteVirtualMemory` are the local allocator's own syscalls — every program
    // that allocates memory imports them, and `KernelBase.dll` imports all three, which
    // made Windows itself read as an injector. The triad worth detecting is MITRE
    // T1055's: `VirtualAllocEx` + `WriteProcessMemory` + a remote-thread primitive.
    ("VirtualAllocEx", cap::ALLOC_REMOTE),
    ("VirtualAllocExNuma", cap::ALLOC_REMOTE),
    ("WriteProcessMemory", cap::WRITE_REMOTE),
    ("CreateRemoteThread", cap::EXEC_REMOTE),
    ("CreateRemoteThreadEx", cap::EXEC_REMOTE),
    ("RtlCreateUserThread", cap::EXEC_REMOTE),
    ("QueueUserAPC", cap::EXEC_REMOTE),
    ("SetThreadContext", cap::EXEC_REMOTE),
    // Anti-analysis.
    ("IsDebuggerPresent", cap::ANTI_DEBUG),
    ("CheckRemoteDebuggerPresent", cap::ANTI_DEBUG),
    ("NtQueryInformationProcess", cap::ANTI_DEBUG),
    ("NtSetInformationThread", cap::ANTI_DEBUG),
    ("OutputDebugStringA", cap::ANTI_DEBUG),
    ("CreateToolhelp32Snapshot", cap::PROCESS_ENUM),
    ("Process32First", cap::PROCESS_ENUM),
    ("Process32FirstW", cap::PROCESS_ENUM),
    ("Process32Next", cap::PROCESS_ENUM),
    ("Process32NextW", cap::PROCESS_ENUM),
    ("EnumProcesses", cap::PROCESS_ENUM),
    ("NtQuerySystemInformation", cap::PROCESS_ENUM),
    ("Module32First", cap::PROCESS_ENUM),
    // Crypto.
    ("CryptEncrypt", cap::CRYPTO),
    ("CryptDecrypt", cap::CRYPTO),
    ("CryptGenKey", cap::CRYPTO),
    ("CryptAcquireContextA", cap::CRYPTO),
    ("CryptAcquireContextW", cap::CRYPTO),
    ("BCryptEncrypt", cap::CRYPTO),
    ("BCryptDecrypt", cap::CRYPTO),
    ("BCryptGenerateSymmetricKey", cap::CRYPTO),
    // Persistence surface.
    ("CreateServiceA", cap::PERSISTENCE),
    ("CreateServiceW", cap::PERSISTENCE),
    ("StartServiceA", cap::PERSISTENCE),
    ("StartServiceW", cap::PERSISTENCE),
    ("ChangeServiceConfigA", cap::PERSISTENCE),
    ("ChangeServiceConfigW", cap::PERSISTENCE),
    ("OpenSCManagerA", cap::PERSISTENCE),
    ("OpenSCManagerW", cap::PERSISTENCE),
];

/// Section names associated with packers and protectors.
///
/// Compared case-insensitively against the section name with trailing spaces
/// stripped (some linkers pad names with spaces rather than NULs).
pub const PACKER_SECTION_NAMES: &[&str] = &[
    "UPX0",
    "UPX1",
    "UPX2",
    "UPX!",
    ".UPX0",
    ".UPX1",
    ".aspack",
    ".ASPack",
    ".adata",
    "ASPack",
    ".nsp0",
    ".nsp1",
    ".nsp2",
    ".packed",
    ".petite",
    ".mpress1",
    ".mpress2",
    ".themida",
    ".winlice",
    ".vmp0",
    ".vmp1",
    ".vmp2",
    ".enigma1",
    ".enigma2",
    ".PECompact",
    ".yP",
    ".boom",
    ".ccg",
    ".charmve",
    ".dcmp",
    ".ecode",
    ".gentee",
    ".kkrunchy",
    ".maskPE",
    ".mew",
    ".neolite",
    ".perplex",
    ".pklstb",
    ".RLPack",
    ".rmnet",
    ".spack",
    ".svkp",
    ".taz",
    ".tElock",
    ".tsuarch",
    ".tsustub",
    ".winapi",
    ".xcr",
    ".y0da",
    ".zeus",
    ".MPRESS1",
    ".MPRESS2",
    ".seau",
    ".shrink1",
    ".shrink2",
    ".shrink3",
];
