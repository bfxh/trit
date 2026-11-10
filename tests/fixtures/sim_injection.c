/* 模拟样本源码（N1 校准批）：全自有代码。
 *
 * 目的 = 让导入表面携带「注入类能力桶」的共现模式（VirtualAllocEx +
 * WriteProcessMemory + CreateRemoteThread + LoadLibraryA），供逆向线的
 * 导入共现判定产生已知裁决。L4 变体在同函数内结构化调用注入类全桶 API
 * （运行时不可达分支——volatile sink 初值 0，静态不触发，结果全部丢弃）；
 * L1-L3 只取指针不调用 = **共现判定的漏报边界**（如实入册：导入共现对
 * "引用而不调用"形态盲——载入级检测是另一层的事）。
 *
 * 编译：x86_64-w64-mingw32-gcc -O1 -DSIM_LEVEL=n sim_injection.c -o sim_lvlN.exe -lkernel32
 * 四个变体的 sha256 见 docs/CALIBRATION.md 首批读数；批次目录 E:/tmp/n1。
 */

#include <windows.h>

/* SIM_LEVEL 控制携带哪些符号；g_sink 是 volatile——编译器不能证明分支
 * 不可达，调用点模式得以保留；运行时 sink 初值 0，永不执行。 */
static volatile void *g_sink[8];

int main(void) {
#if SIM_LEVEL >= 1
    g_sink[0] = (void *)&LoadLibraryA; /* loader 桶：只取指针 */
#endif
#if SIM_LEVEL >= 2
    g_sink[1] = (void *)&VirtualAlloc; /* 分配桶：只取指针（不在桶内——alloc 组要 Ex 版） */
#endif
#if SIM_LEVEL == 3
    g_sink[3] = (void *)&WriteProcessMemory; /* 写桶：只取指针 */
    g_sink[4] = (void *)&CreateRemoteThread; /* 执行桶：只取指针 */
#endif
#if SIM_LEVEL == 4
    /* 检测器验证变体：同函数内结构化调用注入类全桶 API（运行时不可达
     * 分支，结果全部丢弃）。VirtualAlloc 不在 alloc 桶内——alloc 组要
     * VirtualAllocEx（#51 演练同款，本轮差分实测再次确认）。
     * == 精确门：L5 是纯动态解析形态，不能叠加本块。 */
    if (g_sink[1] != 0) {
        HANDLE fake = (HANDLE)(uintptr_t)0x0DeadBeef; /* 无效句柄：运行时永不执行 */
        void *p = VirtualAllocEx(fake, 0, 0x1000, MEM_RESERVE, PAGE_NOACCESS);
        g_sink[5] = p;
        if (p) {
            /* 结构化调用点模式：全部结果丢弃，无任何后续动作 */
            WriteProcessMemory(fake, p, &g_sink[6], 1, 0);
            g_sink[7] = (void *)(uintptr_t)CreateRemoteThread(fake, 0, 0, 0, 0, 0, 0);
        }
    }
#endif
#if SIM_LEVEL >= 5
    /* 逃逸形态变体（dynres 检测链的验证目标）：动态解析——导入表只有
     * 解析器（GetProcAddress），敏感 API 名以字符串字面量出现在数据段
     *（GetProcAddress 取指针，不导入、不调用）。旧检测链对此全盲
     *（fn_injection False——CALIBRATION 首批读数的漏报边界）；
     * dynres 链（resolver ∧ hidden strings）应当 True。 */
    HMODULE k32 = GetModuleHandleA("kernel32.dll");
    FARPROC q1 = GetProcAddress(k32, "VirtualAllocEx");
    FARPROC q2 = GetProcAddress(k32, "WriteProcessMemory");
    FARPROC q3 = GetProcAddress(k32, "CreateRemoteThread");
    g_sink[8] = (void *)((uintptr_t)q1 ^ (uintptr_t)q2 ^ (uintptr_t)q3);
#endif
    return (int)(g_sink[0] != 0);
}
