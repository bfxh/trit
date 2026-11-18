/*
 * 表达式语言 + pe 对象模型的官方对拍规则（docs/YARA-EXPR.md「对拍」）。
 *
 *   python -I scripts/yara_diff.py --trit <trit-re> \
 *       --rules crates/trit-re/tests/fixtures/yara_expr_rules.yar --samples <PE 样本目录>
 *
 * 判据：DIFF 0。规则只用官方 4.5.4 能编译的写法（编译错误类在
 * tests/yara_expr*.rs 的 *_REJECT 表里）；条件尽量挑"有的样本命中、有的不
 * 命中"的，全真 / 全假的规则对拍不出东西。分组前缀对应任务书的 A–E。
 */
import "pe"

/* ---------- A：算术与位运算 ---------- */

rule a_add_sections_imports { condition: pe.number_of_sections + pe.number_of_imports > 12 }
rule a_sub_can_go_negative { condition: pe.number_of_imports - pe.number_of_sections < 0 }
rule a_mul_wraps { condition: filesize * 0x100000000000 > 0 }
rule a_div_truncates { condition: filesize \ 1024 >= 64 and filesize \ 1024 < 256 }
rule a_mod_alignment { condition: filesize % 16 == 8 }
rule a_mod_tail { condition: filesize % 4096 > 2048 }
rule a_and_mask_dll { condition: pe.characteristics & pe.DLL == pe.DLL }
rule a_and_mask_large_address { condition: pe.characteristics & 0x20 != 0 and pe.characteristics & 0x100 == 0 }
rule a_or_then_compare { condition: (pe.characteristics | 0x2000) == pe.characteristics }
rule a_xor_machine { condition: pe.machine ^ 0x8664 == 0 }
rule a_xor_nonzero { condition: (pe.machine ^ pe.MACHINE_ARM64) & 0xff00 == 0x2c00 }
rule a_not_bits { condition: ~pe.characteristics & 0x2000 == 0x2000 }
rule a_shl_scale { condition: pe.number_of_sections << 10 > pe.size_of_headers }
rule a_shr_high_bits { condition: pe.timestamp >> 28 == 6 }
rule a_shift_past_width { condition: filesize >> 64 == 0 and filesize << 64 == 0 }
rule a_unary_minus { condition: -pe.number_of_sections < -6 }
rule a_precedence_mul_before_add { condition: pe.number_of_sections + 2 * 3 == 12 }
rule a_precedence_shift_below_add { condition: 1 << pe.number_of_sections - 4 == 4 }
rule a_precedence_and_over_xor_over_or { condition: pe.subsystem | 4 ^ 6 & 3 == 6 }
rule a_parens_override { condition: (pe.number_of_sections + 2) * 3 == 24 }
rule a_left_assoc_sub { condition: 20 - pe.number_of_sections - 4 == 10 }
rule a_left_assoc_div { condition: filesize \ 1000 \ 10 == 6 }
rule a_runtime_div_zero_is_undefined { condition: filesize \ (pe.number_of_exports - 1) > 0 }
rule a_runtime_mod_zero_is_undefined { condition: not defined (filesize % (pe.number_of_imports - 6)) }
rule a_negative_div_truncates_to_zero { condition: (0 - pe.number_of_sections) \ 4 == -1 }
rule a_negative_mod_keeps_sign { condition: (0 - pe.number_of_sections) % 4 == -2 }
rule a_alignment_round_up { condition: (pe.size_of_headers + pe.file_alignment - 1) & ~(pe.file_alignment - 1) == pe.size_of_headers }
rule a_image_base_high { condition: pe.image_base >> 32 != 0 }
rule a_image_base_low_bits { condition: pe.image_base & 0xffff == 0 and pe.image_base \ 0x10000 % 2 == 0 }
rule a_checksum_parity { condition: pe.checksum % 2 == 1 }
rule a_mixed_bitfield { condition: (pe.dll_characteristics & (pe.DYNAMIC_BASE | pe.NX_COMPAT)) == (pe.DYNAMIC_BASE | pe.NX_COMPAT) and pe.dll_characteristics & pe.GUARD_CF == 0 }
rule a_integer_reads_in_arithmetic { condition: uint16(0) + uint16(2) == 0x5a4d + 0x90 }
rule a_nested_reads { condition: uint32(uint32(0x3c)) == 0x00004550 }
rule a_read_at_computed_offset { condition: uint16(uint32(0x3c) + 4) == pe.machine }
rule a_read_be { condition: uint16be(0) == 0x4d5a and uint32be(uint32(0x3c)) >> 16 == 0x5045 }
rule a_signed_read { condition: int8(uint32(0x3c) + 5) < 0 }
rule a_signed_read_16 { condition: int16(uint32(0x3c) + 4) == pe.machine - 0x10000 }
rule a_read_out_of_range { condition: not defined uint32(filesize - 3) and defined uint32(filesize - 4) }
rule a_filesize_units { condition: filesize > 1MB and filesize < 2MB }
rule a_filesize_kb_expr { condition: filesize \ 1KB + 1 > 100 and filesize - 60KB < 200KB }
rule a_bool_of_integer { condition: pe.number_of_exports - 1 }
rule a_overlay_arithmetic { condition: pe.overlay.offset + pe.overlay.size == filesize and pe.overlay.size > 0x2800 }
rule a_entry_point_inside_first_section { condition: pe.entry_point >= pe.sections[0].raw_data_offset and pe.entry_point < pe.sections[0].raw_data_offset + pe.sections[0].raw_data_size }
rule a_version_packing { condition: pe.linker_version.major * 100 + pe.linker_version.minor >= 1440 }
rule a_os_version { condition: pe.os_version.major * 10 + pe.os_version.minor == 60 and pe.subsystem_version.major == 6 }
rule a_string_counts_in_arithmetic {
  strings:
    $a = "Python"
    $b = "GetProcAddress"
  condition:
    #a + #b * 2 > 6 and #a - #b > 0
}
rule a_string_offsets_in_arithmetic {
  strings:
    $mz = { 4D 5A }
    $pe = { 50 45 00 00 }
  condition:
    @pe[1] - @mz[1] == uint32(0x3c) and @mz[1] + 2 <= filesize
}
rule a_offset_of_missing_occurrence {
  strings:
    $pe = { 50 45 00 00 4C 01 }
  condition:
    not defined @pe[2] and @pe[1] + 0 == uint32(0x3c)
}
rule a_match_at_computed_offset {
  strings:
    $pe = { 50 45 00 00 }
  condition:
    $pe at uint32(0x3c) and not $pe at uint32(0x3c) + 1 and $pe in (uint32(0x3c) - 1 .. uint32(0x3c) + 1)
}

/* ---------- B：变量与区间迭代 ---------- */

rule b_any_section_named_rsrc { condition: for any i in (0..pe.number_of_sections - 1) : ( pe.sections[i].name == ".rsrc" ) }
rule b_all_sections_file_aligned { condition: for all i in (0..pe.number_of_sections - 1) : ( pe.sections[i].raw_data_size % pe.file_alignment == 0 ) }
rule b_count_executable_sections { condition: for 2 i in (0..pe.number_of_sections - 1) : ( pe.sections[i].characteristics & pe.SECTION_MEM_EXECUTE != 0 ) }
rule b_none_writable_and_executable { condition: for none i in (0..pe.number_of_sections - 1) : ( pe.sections[i].characteristics & 0xA0000000 == 0xA0000000 ) }
/* `for 50% i in …` 官方读成 `50 % i` 而报 undefined identifier：百分比量词只能配 `of`（见 b_percent_of） */
rule b_expression_quantifier { condition: for pe.number_of_sections - 1 i in (0..pe.number_of_sections - 1) : ( pe.sections[i].raw_data_size > 0x200 ) }
rule b_range_is_inclusive { condition: for all i in (0..pe.number_of_sections) : ( defined pe.sections[i].name ) }
rule b_range_past_end_is_undefined { condition: for any i in (0..pe.number_of_sections) : ( not defined pe.sections[i].name ) }
rule b_empty_range_is_false { condition: for all i in (pe.number_of_sections..0) : ( true ) }
rule b_single_point_range { condition: for all i in (pe.number_of_sections - 1..pe.number_of_sections - 1) : ( pe.sections[i].name == ".reloc" ) }
rule b_enumerated_values { condition: for any m in (pe.MACHINE_I386, pe.MACHINE_ARM64) : ( pe.machine == m ) }
rule b_enumerated_expressions { condition: for any n in (pe.number_of_sections, pe.number_of_imports, pe.number_of_exports) : ( n == 9 ) }
rule b_byte_scan { condition: for any i in (0..63) : ( uint8(i) == 0xf8 and uint8(i + 1) == 0 ) }
rule b_byte_scan_all { condition: for all i in (0x4e..0x74) : ( uint8(i) >= 0x20 and uint8(i) < 0x7f ) }
rule b_nested_ranges { condition: for any i in (0..pe.number_of_sections - 1) : ( for any j in (i + 1..pe.number_of_sections - 1) : ( pe.sections[i].raw_data_size == pe.sections[j].raw_data_size ) ) }
rule b_nested_three_deep { condition: for any i in (0..2) : ( for any j in (0..2) : ( for any k in (0..2) : ( i + j + k == pe.number_of_sections - 2 ) ) ) }
rule b_outer_variable_in_inner_bound { condition: for all i in (1..pe.number_of_sections - 1) : ( for all j in (0..i - 1) : ( pe.sections[j].virtual_address < pe.sections[i].virtual_address ) ) }
rule b_sections_laid_out_contiguously { condition: for all i in (1..pe.number_of_sections - 1) : ( pe.sections[i].raw_data_offset == pe.sections[i - 1].raw_data_offset + pe.sections[i - 1].raw_data_size ) }
rule b_iterate_sections_directly { condition: for any s in pe.sections : ( s.name == ".pdata" and s.virtual_size > 0x1000 ) }
rule b_iterate_all_sections { condition: for all s in pe.sections : ( s.virtual_address % pe.section_alignment == 0 and s.raw_data_offset >= pe.size_of_headers ) }
rule b_iterate_data_directories { condition: for 6 d in pe.data_directories : ( d.size > 0 ) }
rule b_loop_variable_arithmetic { condition: for any i in (1..8) : ( i * i == pe.number_of_imports ) }
rule b_loop_over_strings_and_range {
  strings:
    $a = "This program cannot be run in DOS mode"
  condition:
    for any i in (1..#a) : ( @a[i] < 0x100 and uint8(@a[i] - 1) != 0x54 )
}
rule b_string_loop_inside_range {
  strings:
    $a = ".text"
    $b = ".rdata"
    $c = ".reloc"
  condition:
    for any i in (0..2) : ( for all of them : ( # > i ) )
}
rule b_range_inside_string_loop {
  strings:
    $a = ".text"
    $b = ".rdata"
  condition:
    for all of them : ( for any i in (1..#) : ( @[i] < 0x400 ) )
}
rule b_of_in_range {
  strings:
    $a = ".text"
    $b = ".rsrc"
    $c = ".pdata"
  condition:
    all of them in (0..0x400)
}
rule b_n_of_with_expression {
  strings:
    $a = ".pdata"
    $b = ".idata"
    $c = ".00cfg"
    $d = "PyRuntim"
  condition:
    pe.number_of_sections - 5 of them
}
rule b_none_of {
  strings:
    $a = ".pdata"
    $b = "PyRuntim"
  condition:
    none of them and pe.is_pe
}
rule b_percent_of {
  strings:
    $a = ".pdata"
    $b = ".idata"
    $c = ".rsrc"
    $d = ".reloc"
  condition:
    75% of them
}

/* ---------- C：动态数组索引 ---------- */

rule c_last_section_by_expression { condition: pe.sections[pe.number_of_sections - 1].name == ".reloc" }
rule c_index_from_function { condition: pe.sections[pe.section_index(".data")].characteristics & pe.SECTION_MEM_WRITE != 0 }
rule c_index_from_entry_point { condition: pe.sections[pe.section_index(pe.entry_point)].name == ".text" }
rule c_out_of_range_is_undefined { condition: not defined pe.sections[pe.number_of_sections].name and not defined pe.sections[0 - 1].name }
rule c_out_of_range_compare_is_false { condition: pe.sections[pe.number_of_sections + 3].raw_data_size == 0 or pe.sections[99].name == ".text" }
rule c_not_over_out_of_range { condition: not ( pe.sections[pe.number_of_sections].raw_data_size > 0 ) }
rule c_index_arithmetic { condition: pe.sections[pe.number_of_sections \ 2].name == ".pdata" }
rule c_index_by_loop_and_compare_names { condition: for any i in (0..pe.number_of_sections - 2) : ( pe.sections[i].name == ".data" and pe.sections[i + 1].name == ".rsrc" ) }
rule c_data_directory_by_constant { condition: pe.data_directories[pe.IMAGE_DIRECTORY_ENTRY_EXPORT].size > 0x100 }
rule c_data_directory_by_expression { condition: pe.data_directories[pe.IMAGE_DIRECTORY_ENTRY_IMPORT + 1].virtual_address == pe.sections[pe.number_of_sections - 2].virtual_address }
rule c_data_directory_tls { condition: pe.data_directories[pe.IMAGE_DIRECTORY_ENTRY_TLS].virtual_address != 0 }
rule c_import_by_index { condition: pe.import_details[pe.number_of_imports - 1].library_name icontains "api-ms-win-crt" }
rule c_import_function_by_two_indexes { condition: pe.import_details[0].functions[pe.import_details[0].number_of_functions - 1].name startswith "Py" }
rule c_export_by_index { condition: pe.export_details[pe.number_of_exports - 1].name startswith "PyInit_" }
rule c_export_ordinal_arithmetic { condition: pe.export_details[0].ordinal + pe.number_of_exports - 1 == pe.export_details[pe.number_of_exports - 1].ordinal }
rule c_resource_by_index { condition: pe.resources[pe.number_of_resources - 1].type == pe.RESOURCE_TYPE_MANIFEST }
rule c_resource_out_of_range { condition: not defined pe.resources[pe.number_of_resources].type and pe.number_of_resources > 0 }
rule c_string_offset_by_expression {
  strings:
    $a = ".text"
  condition:
    #a > 1 and @a[#a] > @a[1] and not defined @a[#a + 1]
}
rule c_string_offset_as_index {
  strings:
    $a = { 50 45 00 00 }
  condition:
    uint16(@a[1] + 6) == pe.number_of_sections and pe.sections[uint16(@a[1] + 6) - 1].raw_data_size > 0
}

/* ---------- D：字符串函数 ---------- */

rule d_contains_section_name { condition: for any s in pe.sections : ( s.name contains "data" ) }
rule d_contains_is_case_sensitive { condition: pe.version_info["OriginalFilename"] contains "Python" }
rule d_icontains { condition: pe.version_info["OriginalFilename"] icontains "PYTHON" }
rule d_startswith { condition: pe.version_info["OriginalFilename"] startswith "_" }
rule d_istartswith { condition: pe.dll_name istartswith "PYTHON3" }
rule d_endswith { condition: pe.version_info["OriginalFilename"] endswith ".pyd" }
rule d_iendswith { condition: pe.dll_name iendswith ".DLL" and not pe.dll_name endswith ".DLL" }
rule d_iequals { condition: pe.version_info["InternalName"] iequals "PYTHON DLL" }
rule d_matches_anchored { condition: pe.version_info["FileVersion"] matches /^3\.12\.[0-9]+$/ }
rule d_matches_unanchored { condition: pe.version_info["LegalCopyright"] matches /Python Software Foundation/ }
rule d_matches_nocase { condition: pe.version_info["CompanyName"] matches /microsoft/i }
rule d_matches_alternation { condition: pe.dll_name matches /^(lib(ssl|crypto)|sqlite3|vcruntime140(_1)?)/ }
rule d_matches_class_and_repeat { condition: for any s in pe.sections : ( s.name matches /^\.[a-z0-9]{6,7}$/ ) }
rule d_matches_on_missing_key { condition: pe.version_info["NoSuchKey"] matches /.*/ }
rule d_undefined_operand_is_undefined { condition: not defined ( pe.version_info["NoSuchKey"] contains "" ) }
rule d_empty_needle { condition: pe.dll_name contains "" and pe.dll_name startswith "" and pe.dll_name endswith "" }
rule d_needle_longer_than_haystack { condition: for any s in pe.sections : ( s.name contains ".rdata.and.then.some" ) }
rule d_string_equality_and_order { condition: pe.sections[0].name == ".text" and pe.sections[0].name < pe.sections[1].name }
rule d_string_inequality { condition: pe.sections[0].name != ".text" }
rule d_string_order_on_names { condition: for all i in (1..pe.number_of_sections - 1) : ( pe.sections[i].name > "." ) and pe.sections[pe.number_of_sections - 1].name >= ".reloc" }
rule d_escapes_in_literals { condition: pe.sections[0].name == "\x2etext" and not pe.sections[0].name contains "\t" }
rule d_import_library_predicate { condition: for any imp in pe.import_details : ( imp.library_name istartswith "vcruntime" and imp.library_name endswith ".dll" ) }
rule d_import_function_predicate { condition: for any imp in pe.import_details : ( for any f in imp.functions : ( f.name matches /^Py(Arg|Err)_/ ) ) }
rule d_export_name_predicate { condition: for any e in pe.export_details : ( e.name iequals "pyinit__socket" or e.name contains "sqlite3_open" ) }
rule d_version_info_iteration { condition: for any k, v in pe.version_info : ( k == "ProductName" and v icontains "openssl" ) }
rule d_version_info_any_value { condition: for any k, v in pe.version_info : ( v matches /^14\.[0-9]+\.[0-9]+\.[0-9]+$/ ) }

/* ---------- E：pe 模块对象 ---------- */

rule e_imports_count { condition: pe.number_of_imports >= 8 and pe.number_of_imports <= 11 }
rule e_imported_functions_total { condition: pe.number_of_imported_functions > 100 }
rule e_imported_functions_sum { condition: pe.number_of_imports > 0 and pe.import_details[0].number_of_functions + pe.import_details[pe.number_of_imports - 1].number_of_functions > 40 }
rule e_imports_dll_function { condition: pe.imports("KERNEL32.dll", "GetModuleHandleW") }
rule e_imports_names_are_case_insensitive { condition: pe.imports("kernel32.DLL", "getmodulehandlew") and not pe.imports("kernel32", "GetModuleHandleW") }
rule e_imports_dll_only { condition: pe.imports("python312.dll") }
rule e_imports_count_form { condition: pe.imports("python312.dll") == 1 and pe.imports("nonexistent.dll") == 0 }
rule e_imports_regex_form { condition: pe.imports(/^api-ms-win-crt-/, /^(malloc|free)$/) >= 2 }
rule e_imports_regex_counts { condition: pe.imports(/python3/i, /^PyUnicode_/) > 5 }
rule e_imports_ordinal_form { condition: pe.imports("WS2_32.dll", 115) }
rule e_import_rva_is_inside_image { condition: pe.number_of_imports > 0 and pe.import_details[0].functions[0].rva < pe.size_of_image and pe.import_details[0].functions[0].rva > 0 }
rule e_import_rva_lookup { condition: pe.number_of_imports > 0 and pe.import_rva(pe.import_details[0].library_name, pe.import_details[0].functions[0].name) == pe.import_details[0].functions[0].rva }
rule e_import_rva_by_name { condition: pe.import_rva("kernel32.dll", "getmodulehandlew") > pe.sections[1].virtual_address }
rule e_import_rva_by_ordinal { condition: pe.import_rva("WS2_32.dll", 115) > 0 }
rule e_import_rva_missing_is_undefined { condition: not defined pe.import_rva("KERNEL32.dll", "NoSuchFunctionAnywhere") and pe.number_of_imports > 0 }
rule e_exports_count { condition: pe.number_of_exports > 100 and pe.number_of_exports < 1000 }
rule e_exports_by_name { condition: pe.exports("Py_Main") }
rule e_exports_by_regex { condition: pe.exports(/^PyInit__(ssl|socket|hashlib)$/) }
rule e_exports_by_ordinal { condition: pe.exports(300) }
rule e_exports_index_by_name { condition: pe.exports_index("Py_Main") >= 0 and pe.export_details[pe.exports_index("Py_Main")].name == "Py_Main" }
rule e_exports_index_missing_is_undefined { condition: not defined pe.exports_index("definitely_not_exported") and pe.number_of_exports > 0 }
rule e_exports_index_by_regex { condition: pe.exports_index(/^sqlite3_/) == 0 }
rule e_export_forwarders { condition: for any e in pe.export_details : ( defined e.forward_name and e.forward_name startswith "python312." ) }
rule e_export_offsets { condition: for all e in pe.export_details : ( not defined e.forward_name and e.offset > 0 and e.offset < filesize ) and pe.number_of_exports > 1 }
rule e_export_timestamp { condition: pe.export_timestamp == 0xFFFFFFFF or pe.export_timestamp == pe.timestamp }
rule e_dll_name_matches_original { condition: pe.dll_name == pe.version_info["OriginalFilename"] }
rule e_resources_count { condition: pe.number_of_resources == 15 }
rule e_resources_version_and_manifest { condition: for any r in pe.resources : ( r.type == pe.RESOURCE_TYPE_VERSION ) and for any r in pe.resources : ( r.type == pe.RESOURCE_TYPE_MANIFEST ) }
rule e_resources_icons { condition: for 7 r in pe.resources : ( r.type == pe.RESOURCE_TYPE_ICON ) }
rule e_resources_lengths { condition: for all r in pe.resources : ( r.length > 0 and r.offset + r.length <= filesize ) and pe.number_of_resources > 0 }
rule e_resources_ids { condition: for any r in pe.resources : ( r.type == pe.RESOURCE_TYPE_MANIFEST and r.id == 2 ) }
rule e_resources_language { condition: for all r in pe.resources : ( r.language == 0x0409 or r.language == 0 ) }
rule e_resources_rva_in_rsrc { condition: for all r in pe.resources : ( r.rva >= pe.sections[pe.section_index(".rsrc")].virtual_address ) and pe.number_of_resources > 1 }
rule e_resource_names_undefined_for_ids { condition: for all r in pe.resources : ( not defined r.type_string and not defined r.name_string ) }
rule e_resource_timestamp_version { condition: pe.resource_timestamp == 0 and pe.resource_version.major == 0 and pe.resource_version.minor == 0 }
rule e_version_info_key { condition: pe.version_info["CompanyName"] == "Python Software Foundation" }
rule e_version_info_missing_key { condition: not defined pe.version_info["CompanyName"] }
rule e_version_info_count { condition: pe.number_of_version_infos >= 8 }
rule e_version_info_list_index { condition: pe.number_of_version_infos > 0 and pe.version_info_list[0].key == "CompanyName" and pe.version_info_list[0].value == pe.version_info["CompanyName"] }
rule e_version_info_list_scan { condition: for any i in (0..pe.number_of_version_infos - 1) : ( pe.version_info_list[i].key == "ProductVersion" and pe.version_info_list[i].value startswith "3.0." ) }
rule e_is_dll_and_exports { condition: pe.is_dll() and pe.number_of_exports == 1 }
rule e_is_64bit { condition: pe.is_64bit() and not pe.is_32bit() and pe.machine != pe.MACHINE_AMD64 }
rule e_exe_with_console { condition: not pe.is_dll() and pe.subsystem == pe.SUBSYSTEM_WINDOWS_CUI }
rule e_opthdr_magic { condition: pe.opthdr_magic == pe.IMAGE_NT_OPTIONAL_HDR32_MAGIC and pe.base_of_data > pe.base_of_code }
rule e_section_index_missing { condition: not defined pe.section_index(".pdata") }
rule e_section_index_by_address { condition: pe.section_index(pe.entry_point) == 0 and pe.sections[0].characteristics & pe.SECTION_CNT_CODE != 0 }
rule e_rva_to_offset { condition: pe.rva_to_offset(pe.sections[0].virtual_address) == pe.sections[0].raw_data_offset and not defined pe.rva_to_offset(pe.size_of_image + 0x100000) }
rule e_entry_point_raw { condition: pe.rva_to_offset(pe.entry_point_raw) == pe.entry_point and pe.entry_point_raw != 0 }
rule e_no_entry_point { condition: pe.entry_point_raw == 0 }
rule e_sizes_add_up { condition: pe.size_of_code + pe.size_of_initialized_data + pe.size_of_headers <= pe.size_of_image + 0x10000 and pe.size_of_uninitialized_data == 0 }
rule e_stack_and_heap { condition: pe.size_of_stack_reserve == 0x1E8480 or pe.size_of_heap_reserve != 0x100000 }
rule e_rva_and_sizes { condition: pe.number_of_rva_and_sizes == 16 and pe.size_of_optional_header == 0xF0 }
rule e_symbols_absent { condition: pe.pointer_to_symbol_table == 0 and pe.number_of_symbols == 0 and pe.win32_version_value == 0 and pe.loader_flags == 0 }
rule e_rich_header_present { condition: defined pe.rich_signature.offset and pe.rich_signature.length >= 0x78 }
rule e_rich_header_key { condition: pe.rich_signature.key != 0 and pe.rich_signature.offset == 0x80 }
rule e_not_a_pe_field_on_pe { condition: pe.is_pe and defined pe.machine }
