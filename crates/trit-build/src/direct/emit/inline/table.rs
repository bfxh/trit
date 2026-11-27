//! 二元表的 C 引用：单表用原名，多表合并进 `T[]` 并写入基址。

/// 一张二元表的 C 引用：名字与索引基址。
#[derive(Clone, Copy)]
pub(crate) struct Table {
    pub(super) name: &'static str,
    pub(super) offset: usize,
}

impl Table {
    pub(super) fn named(name: &'static str) -> Self {
        Self { name, offset: 4 }
    }

    pub(super) fn merged(offset: usize) -> Self {
        Self { name: "T", offset }
    }
}

/// 当前生成物用到的表；单表保持原名，多表合并为 `T`。
#[derive(Clone, Copy, Default)]
pub(crate) struct Tables {
    and: Option<Table>,
    or: Option<Table>,
    xor: Option<Table>,
    xor2: Option<Table>,
    implies: Option<Table>,
    equiv: Option<Table>,
}

impl Tables {
    pub(super) fn named(name: &'static str) -> Self {
        let mut tables = Self::default();
        tables.set(name, Table::named(name));
        tables
    }

    pub(super) fn set(&mut self, name: &str, table: Table) {
        let slot = match name {
            "T_AND" => &mut self.and,
            "T_OR" => &mut self.or,
            "T_XOR" => &mut self.xor,
            "T_XOR2" => &mut self.xor2,
            "T_IMPL" => &mut self.implies,
            "T_EQUIV" => &mut self.equiv,
            other => unreachable!("未知二元表 {other}"),
        };
        *slot = Some(table);
    }

    pub(crate) fn and(&self) -> Table {
        self.and.expect("And 节点没有登记表")
    }

    pub(crate) fn or(&self) -> Table {
        self.or.expect("Or 节点没有登记表")
    }

    pub(crate) fn xor(&self) -> Table {
        self.xor.expect("Xor 节点没有登记表")
    }

    pub(crate) fn xor2(&self) -> Table {
        self.xor2.expect("Xor2 节点没有登记表")
    }

    pub(crate) fn implies(&self) -> Table {
        self.implies.expect("Implies 节点没有登记表")
    }

    pub(crate) fn equiv(&self) -> Table {
        self.equiv.expect("Equiv 节点没有登记表")
    }
}

/// 单表用原名；多表合并成 `T[]`，返回拼好的表值与各算子的基址。
pub(crate) fn tables(entries: &[(&'static str, [i8; 9])]) -> (Tables, Option<Vec<i8>>) {
    if entries.len() <= 1 {
        let tables = match entries.first() {
            Some((name, _)) => Tables::named(name),
            None => Tables::default(),
        };
        return (tables, None);
    }
    let mut tables = Tables::default();
    let mut merged = Vec::new();
    for (name, values) in entries {
        tables.set(name, Table::merged(4 + merged.len()));
        merged.extend_from_slice(values.as_slice());
    }
    (tables, Some(merged))
}

pub(crate) fn call(table: Table, left: String, right: String) -> String {
    format!("{}[3*{left}+{right}+{}]", table.name, table.offset)
}
