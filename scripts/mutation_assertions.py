import re


def baseline_has_tests(output: str) -> bool:
    return any(
        int(match.group(1)) > 0
        for match in re.finditer(r"test result: ok\. (\d+) passed", output)
    )


def failing_blocks(output: str) -> dict[str, str]:
    """cargo 只为失败的测试打印 `---- <name> stdout ----` 块。"""
    pattern = re.compile(
        r"^---- (?P<name>\S+) stdout ----\n(?P<body>.*?)(?=^---- \S+ stdout ----\n|\Z)",
        re.MULTILINE | re.DOTALL,
    )
    return {match.group("name"): match.group("body") for match in pattern.finditer(output)}


def assertion_caught(returncode: int, output: str, test_name: str, diagnostic: str) -> bool:
    """只有「指定的那个测试」的失败块里出现指定断言消息才算检出。

    编译错误、段错误、零测试失败、另一个测试恰好带同一串诊断，都不算。
    单元测试会用 `模块::名字`，所以按后缀匹配。
    """
    if returncode == 0 or "test result: FAILED" not in output:
        return False
    for name, body in failing_blocks(output).items():
        if name == test_name or name.endswith(f"::{test_name}"):
            if "panicked at" in body and diagnostic in body:
                return True
    return False
