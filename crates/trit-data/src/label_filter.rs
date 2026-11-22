//! 标签过滤器（allow / deny）。

#[derive(Debug, Clone, Default)]
pub struct LabelFilter {
    pub enabled: bool,
    pub allow: Option<Vec<String>>,
    pub deny: Vec<String>,
}

impl LabelFilter {
    pub fn denied(&self, label: &str) -> bool {
        if !self.enabled {
            return false;
        }
        self.deny.iter().any(|x| x == label)
            || self
                .allow
                .as_ref()
                .is_some_and(|a| !a.iter().any(|x| x == label))
    }
}
