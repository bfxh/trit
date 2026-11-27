//! 二元表的 C 引用：所有表共用 `T[]`，每张表只记索引基址。

/// 一张二元表在 `T[]` 里的索引基址。
#[derive(Clone, Copy)]
pub(crate) struct Table {
    pub(super) offset: usize,
}

impl Table {
    pub(super) fn at(offset: usize) -> Self {
        Self { offset }
    }
}

/// 当前生成物用到的表，全部指向同一个 `T[]`。
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
    /// 测试用：只登记一张表，基址 4。
    #[cfg(test)]
    pub(super) fn named(name: &'static str) -> Self {
        let mut tables = Self::default();
        tables.set(name, 4);
        tables
    }

    pub(super) fn set(&mut self, name: &str, offset: usize) {
        let slot = match name {
            "T_AND" => &mut self.and,
            "T_OR" => &mut self.or,
            "T_XOR" => &mut self.xor,
            "T_XOR2" => &mut self.xor2,
            "T_IMPL" => &mut self.implies,
            "T_EQUIV" => &mut self.equiv,
            other => unreachable!("未知二元表 {other}"),
        };
        *slot = Some(Table::at(offset));
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

/// 把用到的表按固定顺序拼成一个 `T[]`，返回各算子基址与表值。
pub(crate) fn tables(entries: &[(&'static str, [i8; 9])]) -> (Tables, Vec<i8>) {
    let mut tables = Tables::default();
    let mut merged = Vec::new();
    for (name, values) in entries {
        tables.set(name, 4 + merged.len());
        merged.extend_from_slice(values.as_slice());
    }
    (tables, merged)
}

pub(crate) fn call(table: Table, left: String, right: String) -> String {
    format!("T[3*{left}+{right}+{}]", table.offset)
}
