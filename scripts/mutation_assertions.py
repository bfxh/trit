import re


def baseline_has_tests(output: str) -> bool:
    return any(
        int(match.group(1)) > 0
        for match in re.finditer(r"test result: ok\. (\d+) passed", output)
    )


def assertion_caught(returncode: int, output: str, test_name: str, diagnostic: str) -> bool:
    failed_test = re.search(
        rf"^(?:test )?{re.escape(test_name)} (?:---|\.\.\.) FAILED$",
        output,
        re.MULTILINE,
    )
    failed_summary = re.search(r"test result: FAILED\. \d+ passed; [1-9]\d* failed", output)
    return bool(
        returncode != 0
        and failed_test
        and failed_summary
        and "panicked at" in output
        and diagnostic in output
    )
