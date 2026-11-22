//! format 单测。

    use super::*;

    #[test]
    fn detects_structured_formats() {
        assert!(detect(r#"{"a": 1}"#).contains(&"json"));
        assert!(detect("{\"a\":1}\n{\"b\":2}").contains(&"jsonl"));
        assert!(detect("name: trit\nversion: 1\n---\n").contains(&"yaml"));
        assert!(detect("[pkg]\nname = \"trit\"\n").contains(&"toml"));
        assert!(detect("<root><a>1</a></root>").contains(&"xml"));
        assert!(detect("<html><body></body></html>").contains(&"html"));
        assert!(detect("a,b,c\n1,2,3\n").contains(&"csv"));
        assert!(detect("# Title\n- item\n").contains(&"markdown"));
        assert!(detect("SELECT * FROM t;").contains(&"sql"));
        assert!(detect(r"\documentclass{article}").contains(&"latex"));
        assert!(detect("FROM ubuntu\nRUN echo hi\n").contains(&"dockerfile"));
        assert!(detect("query { user { id } }").contains(&"graphql"));
        assert!(detect("a=1 b=2\nc=3 d=4\n").contains(&"logfmt"));
        assert!(detect("diff --git a b\n@@ -1 +1 @@\n").contains(&"diff"));
    }

    #[test]
    fn plain_text_has_no_structured_format() {
        let out = detect("这是一段普通的自然语言说明。");
        assert!(out.is_empty(), "{out:?}");
    }
