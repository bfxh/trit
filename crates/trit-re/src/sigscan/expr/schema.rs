//! `pe` 模块的**类型表**（照 `pe.c` 的 `begin_declarations` 抄录）——解析期
//! 用它做官方同款的静态检查：字段不存在、把函数当字段用、下标类型不对，
//! 官方都是编译错误，这里对应整条规则 `Unsupported`。
//!
//! 类型表**宽于**求值子集：表里有而对象树没建模的字段可以通过解析，
//! 求值时记名弃权（`Unknown`）——`signatures[]`（Authenticode）整族就是
//! 这样：类型表照抄，对象树不建模。

use super::peconst::CONSTS;

/// 静态类型。
#[derive(Debug)]
pub(crate) enum Sch {
    Int,
    Str,
    Struct(&'static [(&'static str, Sch)]),
    Array(&'static Sch),
    /// 字符串键字典（值类型）。
    Dict(&'static Sch),
    /// 重载表：`(实参签名, 返回字符串?)`，签名字符 `i`/`s`/`r`。
    Func(&'static [(&'static str, bool)]),
}

const VERSION: Sch = Sch::Struct(&[("major", Sch::Int), ("minor", Sch::Int)]);
const FLAG: Sch = Sch::Func(&[("", false)]);
const BY_NAME_OR_ORDINAL: Sch = Sch::Func(&[("ss", false), ("si", false)]);
const RICH_LOOKUP: Sch = Sch::Func(&[("i", false), ("ii", false)]);

static DIRECTORY: Sch = Sch::Struct(&[("virtual_address", Sch::Int), ("size", Sch::Int)]);

static SECTION: Sch = Sch::Struct(&[
    ("name", Sch::Str),
    ("full_name", Sch::Str),
    ("characteristics", Sch::Int),
    ("virtual_address", Sch::Int),
    ("virtual_size", Sch::Int),
    ("raw_data_offset", Sch::Int),
    ("raw_data_size", Sch::Int),
    ("pointer_to_relocations", Sch::Int),
    ("pointer_to_line_numbers", Sch::Int),
    ("number_of_relocations", Sch::Int),
    ("number_of_line_numbers", Sch::Int),
]);

static KEY_VALUE: Sch = Sch::Struct(&[("key", Sch::Str), ("value", Sch::Str)]);

static EXPORT: Sch = Sch::Struct(&[
    ("offset", Sch::Int),
    ("name", Sch::Str),
    ("forward_name", Sch::Str),
    ("ordinal", Sch::Int),
    ("rva", Sch::Int),
]);

static IMPORT_FUNCTION: Sch =
    Sch::Struct(&[("name", Sch::Str), ("ordinal", Sch::Int), ("rva", Sch::Int)]);

static IMPORT: Sch = Sch::Struct(&[
    ("library_name", Sch::Str),
    ("number_of_functions", Sch::Int),
    ("functions", Sch::Array(&IMPORT_FUNCTION)),
]);

static RESOURCE: Sch = Sch::Struct(&[
    ("rva", Sch::Int),
    ("offset", Sch::Int),
    ("length", Sch::Int),
    ("type", Sch::Int),
    ("id", Sch::Int),
    ("language", Sch::Int),
    ("type_string", Sch::Str),
    ("name_string", Sch::Str),
    ("language_string", Sch::Str),
]);

static STRING: Sch = Sch::Str;

/// 证书（`signatures[].certificates[]` 与两处 `chain[]` 同形）。
static CERT: Sch = Sch::Struct(&[
    ("thumbprint", Sch::Str),
    ("issuer", Sch::Str),
    ("subject", Sch::Str),
    ("version", Sch::Int),
    ("algorithm", Sch::Str),
    ("algorithm_oid", Sch::Str),
    ("serial", Sch::Str),
    ("not_before", Sch::Int),
    ("not_after", Sch::Int),
]);

static COUNTERSIGNATURE: Sch = Sch::Struct(&[
    ("verified", Sch::Int),
    ("sign_time", Sch::Int),
    ("digest_alg", Sch::Str),
    ("digest", Sch::Str),
    ("length_of_chain", Sch::Int),
    ("chain", Sch::Array(&CERT)),
]);

static SIGNATURE: Sch = Sch::Struct(&[
    ("thumbprint", Sch::Str),
    ("issuer", Sch::Str),
    ("subject", Sch::Str),
    ("version", Sch::Int),
    ("algorithm", Sch::Str),
    ("algorithm_oid", Sch::Str),
    ("serial", Sch::Str),
    ("not_before", Sch::Int),
    ("not_after", Sch::Int),
    ("verified", Sch::Int),
    ("digest_alg", Sch::Str),
    ("digest", Sch::Str),
    ("file_digest", Sch::Str),
    ("number_of_certificates", Sch::Int),
    ("certificates", Sch::Array(&CERT)),
    (
        "signer_info",
        Sch::Struct(&[
            ("program_name", Sch::Str),
            ("digest", Sch::Str),
            ("digest_alg", Sch::Str),
            ("length_of_chain", Sch::Int),
            ("chain", Sch::Array(&CERT)),
        ]),
    ),
    ("number_of_countersignatures", Sch::Int),
    ("countersignatures", Sch::Array(&COUNTERSIGNATURE)),
    ("valid_on", Sch::Func(&[("i", false)])),
]);

/// `pe` 模块根。
pub(crate) static PE: Sch = Sch::Struct(&[
    ("is_pe", Sch::Int),
    ("machine", Sch::Int),
    ("number_of_sections", Sch::Int),
    ("timestamp", Sch::Int),
    ("pointer_to_symbol_table", Sch::Int),
    ("number_of_symbols", Sch::Int),
    ("size_of_optional_header", Sch::Int),
    ("characteristics", Sch::Int),
    ("entry_point", Sch::Int),
    ("entry_point_raw", Sch::Int),
    ("image_base", Sch::Int),
    ("number_of_rva_and_sizes", Sch::Int),
    ("number_of_version_infos", Sch::Int),
    ("version_info", Sch::Dict(&STRING)),
    ("version_info_list", Sch::Array(&KEY_VALUE)),
    ("opthdr_magic", Sch::Int),
    ("size_of_code", Sch::Int),
    ("size_of_initialized_data", Sch::Int),
    ("size_of_uninitialized_data", Sch::Int),
    ("base_of_code", Sch::Int),
    ("base_of_data", Sch::Int),
    ("section_alignment", Sch::Int),
    ("file_alignment", Sch::Int),
    ("linker_version", VERSION),
    ("os_version", VERSION),
    ("image_version", VERSION),
    ("subsystem_version", VERSION),
    ("win32_version_value", Sch::Int),
    ("size_of_image", Sch::Int),
    ("size_of_headers", Sch::Int),
    ("checksum", Sch::Int),
    ("calculate_checksum", FLAG),
    ("subsystem", Sch::Int),
    ("dll_characteristics", Sch::Int),
    ("size_of_stack_reserve", Sch::Int),
    ("size_of_stack_commit", Sch::Int),
    ("size_of_heap_reserve", Sch::Int),
    ("size_of_heap_commit", Sch::Int),
    ("loader_flags", Sch::Int),
    ("data_directories", Sch::Array(&DIRECTORY)),
    ("sections", Sch::Array(&SECTION)),
    (
        "overlay",
        Sch::Struct(&[("offset", Sch::Int), ("size", Sch::Int)]),
    ),
    (
        "rich_signature",
        Sch::Struct(&[
            ("offset", Sch::Int),
            ("length", Sch::Int),
            ("key", Sch::Int),
            ("raw_data", Sch::Str),
            ("clear_data", Sch::Str),
            ("version_data", Sch::Str),
            ("version", RICH_LOOKUP),
            ("toolid", RICH_LOOKUP),
        ]),
    ),
    ("imphash", Sch::Func(&[("", true)])),
    ("section_index", Sch::Func(&[("s", false), ("i", false)])),
    (
        "exports",
        Sch::Func(&[("s", false), ("r", false), ("i", false)]),
    ),
    (
        "exports_index",
        Sch::Func(&[("s", false), ("i", false), ("r", false)]),
    ),
    (
        "imports",
        Sch::Func(&[
            ("ss", false),
            ("si", false),
            ("s", false),
            ("rr", false),
            ("iss", false),
            ("isi", false),
            ("is", false),
            ("irr", false),
        ]),
    ),
    ("import_rva", BY_NAME_OR_ORDINAL),
    ("delayed_import_rva", BY_NAME_OR_ORDINAL),
    ("locale", Sch::Func(&[("i", false)])),
    ("language", Sch::Func(&[("i", false)])),
    ("is_dll", FLAG),
    ("is_32bit", FLAG),
    ("is_64bit", FLAG),
    ("number_of_imports", Sch::Int),
    ("number_of_imported_functions", Sch::Int),
    ("number_of_delayed_imports", Sch::Int),
    ("number_of_delayed_imported_functions", Sch::Int),
    ("number_of_exports", Sch::Int),
    ("dll_name", Sch::Str),
    ("export_timestamp", Sch::Int),
    ("export_details", Sch::Array(&EXPORT)),
    ("import_details", Sch::Array(&IMPORT)),
    ("delayed_import_details", Sch::Array(&IMPORT)),
    ("resource_timestamp", Sch::Int),
    ("resource_version", VERSION),
    ("resources", Sch::Array(&RESOURCE)),
    ("number_of_resources", Sch::Int),
    ("pdb_path", Sch::Str),
    ("is_signed", Sch::Int),
    ("signatures", Sch::Array(&SIGNATURE)),
    ("number_of_signatures", Sch::Int),
    ("rva_to_offset", Sch::Func(&[("i", false)])),
]);

impl Sch {
    /// 结构体字段查找：返回表内的 `'static` 名字与类型。
    pub(crate) fn field(&'static self, name: &str) -> Option<(&'static str, &'static Sch)> {
        match self {
            Sch::Struct(fields) => fields
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(key, ty)| (*key, ty)),
            _ => None,
        }
    }
}

/// `pe.<全大写常量>` 的值。
pub(crate) fn constant(name: &str) -> Option<i64> {
    CONSTS
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, value)| *value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_probed_values() {
        assert_eq!(CONSTS.len(), 171);
        assert_eq!(constant("MACHINE_AMD64"), Some(0x8664));
        assert_eq!(constant("MACHINE_I386"), Some(0x14c));
        assert_eq!(constant("DLL"), Some(0x2000));
        assert_eq!(constant("SECTION_MEM_EXECUTE"), Some(0x2000_0000));
        assert_eq!(constant("RESOURCE_TYPE_VERSION"), Some(16));
        assert_eq!(constant("IMPORT_ANY"), Some(-1));
        assert_eq!(constant("NOPE"), None);
    }

    #[test]
    fn schema_knows_official_names_only() {
        let (_, sections) = PE.field("sections").expect("sections 在表内");
        let Sch::Array(section) = sections else {
            panic!("sections 应是数组");
        };
        // 官方字段名是 raw_data_size / raw_data_offset（探针：raw_size 是
        // 编译错误 invalid field name）
        assert!(section.field("raw_data_size").is_some());
        assert!(section.field("raw_size").is_none());
        assert!(matches!(PE.field("is_dll"), Some((_, Sch::Func(_)))));
        // Authenticode 整族：类型表里有（解析通过），对象树不建模（求值弃权）
        let (_, signatures) = PE.field("signatures").expect("signatures");
        let Sch::Array(signature) = signatures else {
            panic!("signatures 应是数组")
        };
        assert!(matches!(
            signature.field("valid_on"),
            Some((_, Sch::Func(_)))
        ));
        assert!(matches!(
            signature.field("signer_info"),
            Some((_, Sch::Struct(_)))
        ));
    }
}
