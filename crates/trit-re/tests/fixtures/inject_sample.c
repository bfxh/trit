/*
 * 真实 PE 的 `fn_injection` 正例（复算用，不参与构建）。
 *
 * 构建（本机 MSYS2 mingw64 gcc 16.2.0）：
 *   gcc -O1 -o inject.exe inject_sample.c
 *
 * 期望读数（`trit-re triage inject.exe --functions`）：
 *   - `fn_injection = True`，对应函数里同时出现
 *     `imports.inj.alloc` / `imports.inj.write` / `imports.inj.exec`；
 *   - `injection = True`（导入层面三件套齐备）；
 *   - `fn_stager = False`。
 *
 * 说明：`-O1` 下三个调用留在同一个函数里；更高的优化级别可能把它们拆开或内联，
 * 那时 `fn_injection` 的结果应按实际边界重新读，不能沿用本文的期望值。
 */
#include <windows.h>

int main(void) {
    DWORD pid = 4242;
    HANDLE process = OpenProcess(PROCESS_ALL_ACCESS, FALSE, pid);
    LPVOID remote = VirtualAllocEx(process, NULL, 4096, MEM_COMMIT | MEM_RESERVE,
                                   PAGE_EXECUTE_READWRITE);
    const char payload[] = "payload";
    SIZE_T written = 0;
    WriteProcessMemory(process, remote, payload, sizeof(payload), &written);
    HANDLE thread = CreateRemoteThread(process, NULL, 0, NULL, NULL, 0, NULL);
    if (thread != NULL) {
        WaitForSingleObject(thread, 1000);
        CloseHandle(thread);
    }
    return (remote != NULL) ? 0 : 1;
}
