//! 内容门裁决。

use crate::content::Content;
use crate::semantics::SemanticKind;
use crate::style::StyleKind;

#[derive(Debug, Clone, PartialEq)]
pub struct ContentVerdict {
    pub content: Content,
    pub style: StyleKind,
    pub semantics: SemanticKind,
    pub code_filtered: bool,
    pub natural_filtered: bool,
    pub lang_denied: bool,
    pub style_filtered: bool,
    pub semantics_filtered: bool,
    pub code_quality_filtered: bool,
    pub edu_filtered: bool,
    pub formats: Vec<&'static str>,
    pub format_filtered: bool,
    pub bits: Vec<&'static str>,
    pub bits_filtered: bool,
}

impl ContentVerdict {
    pub fn filtered(&self) -> bool {
        self.code_filtered
            || self.natural_filtered
            || self.lang_denied
            || self.style_filtered
            || self.semantics_filtered
            || self.code_quality_filtered
            || self.edu_filtered
            || self.format_filtered
            || self.bits_filtered
    }
}
