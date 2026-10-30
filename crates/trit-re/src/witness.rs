//! 事实表：每条逆向观测都是一个**带值的三值断言**。
//!
//! 三值不是装饰：解析器拿不到证据时必须给 `Unknown`，而不是默认 `False`。
//! 例如导入了 `VirtualAllocEx` 就是 `True`；解析了完整导入表但没有它是
//! `False`；节表被破坏、导入表读不出来才是 `Unknown`。
//!
//! 事实的 id 是稳定字符串（会进 JSONL 台账），detail 只给人看。

use std::collections::BTreeMap;

use trit_belnap::B4;
use trit_core::value::Ternary;

/// 一条可复算的观测。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Witness {
    /// 稳定 id，形如 `section.high_entropy`。
    pub id: &'static str,
    /// 三值结论。
    pub value: Ternary,
    /// 人读证据（哈希、路径、命中名字等）。
    pub detail: String,
}

/// 有序事实表（BTreeMap 保证台账行稳定）。
#[derive(Debug, Default, Clone)]
pub struct Facts {
    entries: BTreeMap<&'static str, Witness>,
}

impl Facts {
    /// 空表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 写入/覆盖一条事实。
    pub fn set(&mut self, id: &'static str, value: Ternary, detail: impl Into<String>) {
        self.entries.insert(
            id,
            Witness {
                id,
                value,
                detail: detail.into(),
            },
        );
    }

    /// 便捷：布尔 → 三值。
    pub fn set_bool(&mut self, id: &'static str, value: bool, detail: impl Into<String>) {
        self.set(id, Ternary::from(value), detail);
    }

    /// 取值；未登记返回 `None`。
    pub fn get(&self, id: &str) -> Option<Ternary> {
        self.entries.get(id).map(|witness| witness.value)
    }

    /// 按变量名取值，未登记按 `Unknown` 处理（与 `trit-eval` 的约定一致）。
    pub fn value_or_unknown(&self, id: &str) -> Ternary {
        self.get(id).unwrap_or(Ternary::Unknown)
    }

    /// 取完整证据。
    pub fn witness(&self, id: &str) -> Option<&Witness> {
        self.entries.get(id)
    }

    /// 迭代全部事实（按 id 排序）。
    pub fn iter(&self) -> impl Iterator<Item = &Witness> {
        self.entries.values()
    }

    /// 事实条数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 投影成 Belnap 四值赋值：`T→T`、`F→F`、`Unknown→N`。
    pub fn belnap_assignment(&self) -> BTreeMap<String, B4> {
        self.entries
            .iter()
            .map(|(id, witness)| ((*id).to_string(), B4::from(witness.value)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Facts;
    use trit_belnap::B4;
    use trit_core::value::Ternary;

    #[test]
    fn missing_fact_reads_unknown() {
        let facts = Facts::new();
        assert_eq!(facts.get("nope"), None);
        assert_eq!(facts.value_or_unknown("nope"), Ternary::Unknown);
    }

    #[test]
    fn overwrite_keeps_single_entry() {
        let mut facts = Facts::new();
        facts.set_bool("a", true, "first");
        facts.set_bool("a", false, "second");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts.get("a"), Some(Ternary::False));
        assert_eq!(facts.witness("a").unwrap().detail, "second");
    }

    #[test]
    fn belnap_projection_maps_n_to_unknown() {
        let mut facts = Facts::new();
        facts.set_bool("t", true, "");
        facts.set_bool("f", false, "");
        facts.set("n", Ternary::Unknown, "");
        let map = facts.belnap_assignment();
        assert_eq!(map.get("t"), Some(&B4::T));
        assert_eq!(map.get("f"), Some(&B4::F));
        assert_eq!(map.get("n"), Some(&B4::N));
    }
}
