//! 文档 / 演示 / 表格 / PDF 格式识别。

pub fn detect(text: &str) -> Vec<&'static str> {
    let lower = text.to_lowercase();
    let mut out = Vec::new();
    if lower.contains("%pdf-") {
        out.push("pdf");
    }
    if lower.contains("ppt/presentation.xml") || lower.contains("<p:sld") || lower.contains("<a:t>") {
        out.push("pptx");
    }
    if lower.contains("word/document.xml") || lower.contains("<w:t>") {
        out.push("docx");
    }
    if lower.contains("xl/workbook.xml") || lower.contains("<c:sheet") {
        out.push("xlsx");
    }
    out
}
