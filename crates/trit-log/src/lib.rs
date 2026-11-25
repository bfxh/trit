//! trit 统一运行时日志。
//!
//! 目标只有一条：全仓的诊断输出走同一个开关，不再散落成 `println!` / `eprintln!`。
//! 结果输出（表格、JSON、上屏正文）仍然用 `println!`，不要经这里——日志和结果是两件事。
//!
//! # 用法
//!
//! ```no_run
//! trit_log::init();                       // 读环境变量，进程里调一次即可（重复调用无害）
//! trit_log::info!("加载模型 {}", "x.dcn");
//! trit_log::warn!("缺少 {}", "可选文件");
//! ```
//!
//! # 环境变量
//!
//! - `TRIT_LOG`：级别或过滤表。`error|warn|info|debug|trace|off`，
//!   也可以写 `trit_parts=debug,trit_parts::train=trace`；最长前缀胜出，`-` 按 `_` 归一。
//! - `TRIT_LOG_FILE`：写文件（追加）；不设时写 stderr。
//! - `TRIT_LOG_FORMAT`：`text`（默认）或 `jsonl`。
//!
//! 默认级别是 `info`：迁移前的 `println!` / `eprintln!` 都看得见，行为不缩水。
//! 日志器自身不会 panic，也不会因为写文件失败而影响调用方。

#![forbid(unsafe_code)]

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// 日志级别。`Off` 用来整体关闭，数值越大越啰嗦。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// 关闭全部日志。
    Off = 0,
    /// 错误：这次操作失败或结果不可信。
    Error = 1,
    /// 警告：能继续，但有东西不对。
    Warn = 2,
    /// 信息：正常进度与关键状态。
    Info = 3,
    /// 调试：排查问题时才需要的过程细节。
    Debug = 4,
    /// 追踪：逐条数据流，量最大。
    Trace = 5,
}

impl Level {
    /// 级别名（小写，JSON 与过滤表用这个口径）。
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Off => "off",
            Level::Error => "error",
            Level::Warn => "warn",
            Level::Info => "info",
            Level::Debug => "debug",
            Level::Trace => "trace",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Level::Off => "OFF  ",
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 解析单个级别名；认不出返回 `None`（调用方决定回退，不在这里猜）。
pub fn parse_level(text: &str) -> Option<Level> {
    match text.trim().to_ascii_lowercase().as_str() {
        "off" | "none" | "silent" => Some(Level::Off),
        "error" | "err" => Some(Level::Error),
        "warn" | "warning" => Some(Level::Warn),
        "info" => Some(Level::Info),
        "debug" => Some(Level::Debug),
        "trace" => Some(Level::Trace),
        _ => None,
    }
}

/// 输出格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// 人读：`<ts> <LEVEL> <target> - <msg>`。
    Text,
    /// 机读：一行一个 JSON 对象。
    Jsonl,
}

impl Format {
    /// 解析格式名；认不出返回 `None`。
    pub fn parse(text: &str) -> Option<Format> {
        match text.trim().to_ascii_lowercase().as_str() {
            "text" | "plain" => Some(Format::Text),
            "json" | "jsonl" => Some(Format::Jsonl),
            _ => None,
        }
    }
}

struct State {
    default: Level,
    filters: Vec<(String, Level)>,
    format: Format,
    file: Option<File>,
}

impl State {
    fn new() -> State {
        State {
            default: Level::Info,
            filters: Vec::new(),
            format: Format::Text,
            file: None,
        }
    }

    fn from_env() -> State {
        let mut state = State::new();
        if let Ok(spec) = std::env::var("TRIT_LOG") {
            state.apply_spec(&spec);
        }
        if let Some(parsed) = std::env::var("TRIT_LOG_FORMAT")
            .ok()
            .and_then(|text| Format::parse(&text))
        {
            state.format = parsed;
        }
        if let Some(path) = std::env::var("TRIT_LOG_FILE")
            .ok()
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
        {
            state.file = open_sink(Path::new(&path));
        }
        state
    }

    fn apply_spec(&mut self, spec: &str) {
        for part in spec.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            match part.split_once('=') {
                None => {
                    if let Some(level) = parse_level(part) {
                        self.default = level;
                    }
                }
                Some((target, level)) => {
                    if let (false, Some(level)) = (target.trim().is_empty(), parse_level(level)) {
                        self.filters.push((normalize_target(target.trim()), level));
                    }
                }
            }
        }
        self.filters
            .sort_by_key(|(target, _)| std::cmp::Reverse(target.len()));
    }

    fn effective(&self, target: &str) -> Level {
        effective_level(&self.filters, self.default, target)
    }

    fn enabled(&self, level: Level, target: &str) -> bool {
        let current = self.effective(target);
        current != Level::Off && level <= current && level != Level::Off
    }

    fn render(&self, level: Level, target: &str, msg: &str, now_ms: u128) -> String {
        match self.format {
            Format::Text => format!("{} {} {} - {}", iso8601(now_ms), level.label(), target, msg),
            Format::Jsonl => format!(
                "{{\"ts\":\"{}\",\"level\":\"{}\",\"target\":\"{}\",\"msg\":\"{}\"}}",
                iso8601(now_ms),
                level.as_str(),
                escape_json(target),
                escape_json(msg)
            ),
        }
    }
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::from_env()));

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 重新读环境变量（`init` 的显式版本）。重复调用无害，已有文件句柄会被替换。
pub fn init() {
    let fresh = State::from_env();
    *state() = fresh;
}

/// 改全局默认级别；过滤表仍然优先。
pub fn set_level(level: Level) {
    state().default = level;
}

/// 当前默认级别。
pub fn level() -> Level {
    state().default
}

/// 改输出格式。
pub fn set_format(format: Format) {
    state().format = format;
}

/// 写文件（追加）；传 `None` 回到 stderr。
pub fn set_file(path: Option<&Path>) -> std::io::Result<()> {
    let mut guard = state();
    guard.file = match path {
        None => None,
        Some(path) => Some(open_sink(path).ok_or_else(|| {
            std::io::Error::other(format!("无法打开日志文件 {}", path.display()))
        })?),
    };
    Ok(())
}

/// 某个 target 当前是否会记录 `level`。
pub fn enabled(level: Level, target: &str) -> bool {
    state().enabled(level, target)
}

/// 真正的写入口；宏用它，手写调用也可以。
pub fn log(level: Level, target: &str, args: fmt::Arguments<'_>) {
    if level == Level::Off {
        return;
    }
    let mut guard = state();
    if !guard.enabled(level, target) {
        return;
    }
    let msg = fmt::format(args);
    let line = guard.render(level, target, &msg, now_millis());
    match guard.file.as_mut() {
        Some(file) => {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
        None => eprintln!("{line}"),
    }
}

fn open_sink(path: &Path) -> Option<File> {
    OpenOptions::new().create(true).append(true).open(path).ok()
}

fn normalize_target(target: &str) -> String {
    target.trim().replace('-', "_")
}

/// 过滤表求值：最长前缀胜出，前缀按 `::` 边界匹配。
fn effective_level(filters: &[(String, Level)], default: Level, target: &str) -> Level {
    let norm = normalize_target(target);
    for (prefix, level) in filters {
        if norm == *prefix || norm.starts_with(&format!("{prefix}::")) {
            return *level;
        }
    }
    default
}

fn escape_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// UTC 时间戳：`YYYY-MM-DDTHH:MM:SS.mmmZ`，不引第三方时间库。
fn iso8601(ms: u128) -> String {
    let secs = (ms / 1000) as i64;
    let millis = (ms % 1000) as u32;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// 天数（1970-01-01 起）转公历，Howard Hinnant 的 `civil_from_days`。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    (y + i64::from(m <= 2), m as u32, d as u32)
}

/// 记录一条 `error`。
#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
        $crate::log($crate::Level::Error, module_path!(), format_args!($($arg)*))
    };
}

/// 记录一条 `warn`。
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        $crate::log($crate::Level::Warn, module_path!(), format_args!($($arg)*))
    };
}

/// 记录一条 `info`。
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        $crate::log($crate::Level::Info, module_path!(), format_args!($($arg)*))
    };
}

/// 记录一条 `debug`。
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        $crate::log($crate::Level::Debug, module_path!(), format_args!($($arg)*))
    };
}

/// 记录一条 `trace`。
#[macro_export]
macro_rules! trace {
    ($($arg:tt)*) => {
        $crate::log($crate::Level::Trace, module_path!(), format_args!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filters(pairs: &[(&str, Level)]) -> Vec<(String, Level)> {
        let mut out: Vec<(String, Level)> = pairs
            .iter()
            .map(|(target, level)| (normalize_target(target), *level))
            .collect();
        out.sort_by_key(|(target, _)| std::cmp::Reverse(target.len()));
        out
    }

    #[test]
    fn parses_every_level_and_alias() {
        assert_eq!(parse_level("ERROR"), Some(Level::Error));
        assert_eq!(parse_level("warning"), Some(Level::Warn));
        assert_eq!(parse_level("off"), Some(Level::Off));
        assert_eq!(parse_level("nope"), None);
    }

    #[test]
    fn default_is_info() {
        assert_eq!(State::new().default, Level::Info);
    }

    #[test]
    fn filter_spec_parses_bare_and_scoped() {
        let mut state = State::new();
        state.apply_spec("warn, trit-parts=debug, trit_parts::train = trace, broken, =nope");
        assert_eq!(state.default, Level::Warn);
        assert_eq!(
            effective_level(&state.filters, state.default, "trit_parts"),
            Level::Debug
        );
        assert_eq!(
            effective_level(&state.filters, state.default, "trit_parts::train"),
            Level::Trace
        );
        assert_eq!(
            effective_level(&state.filters, state.default, "trit_parts::train::step"),
            Level::Trace
        );
        assert_eq!(
            effective_level(&state.filters, state.default, "trit_core"),
            Level::Warn
        );
    }

    #[test]
    fn longest_prefix_wins() {
        let f = filters(&[
            ("trit_parts", Level::Debug),
            ("trit_parts::train", Level::Trace),
        ]);
        assert_eq!(
            effective_level(&f, Level::Info, "trit_parts::train"),
            Level::Trace
        );
        assert_eq!(
            effective_level(&f, Level::Info, "trit_parts::model"),
            Level::Debug
        );
        assert_eq!(effective_level(&f, Level::Info, "trit_partsx"), Level::Info);
    }

    #[test]
    fn render_text_has_utc_stamp_and_level() {
        let state = State::new();
        assert_eq!(
            state.render(Level::Warn, "trit_parts::train", "坏批", 0),
            "1970-01-01T00:00:00.000Z WARN  trit_parts::train - 坏批"
        );
        assert_eq!(iso8601(1_700_000_000_123), "2023-11-14T22:13:20.123Z");
    }

    #[test]
    fn render_jsonl_escapes() {
        let mut state = State::new();
        state.format = Format::Jsonl;
        let line = state.render(Level::Error, "trit::x", "引号\" 反斜杠\\ 换行\n", 0);
        assert_eq!(
            line,
            "{\"ts\":\"1970-01-01T00:00:00.000Z\",\"level\":\"error\",\"target\":\"trit::x\",\"msg\":\"引号\\\" 反斜杠\\\\ 换行\\n\"}"
        );
        assert!(!line.contains('\n'));
    }

    #[test]
    fn civil_dates_round_trip_known_points() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn off_silences_everything() {
        let mut state = State::new();
        state.default = Level::Off;
        assert!(!state.enabled(Level::Error, "trit::x"));
    }
}
