/* TRB2 主机侧运行时（M2）：解码 + 校验，不含求值。
 *
 * 求值走 `lookup.c`（`trit-build` 直接产出的表）；这里只负责把 TRB2 blob
 * 还原成逐 trit 的数组，并**独立**复算 GF(3) CRC-3 —— 与 Rust 实现互相验证，
 * 避免"编码和解码一起错"的假通过。
 */
#ifndef TRIT_RUNTIME_H
#define TRIT_RUNTIME_H

#include <stddef.h>
#include <stdint.h>

#define TRIT_FALSE (-1)
#define TRIT_UNKNOWN 0
#define TRIT_TRUE 1

/* 错误码（<0）；0 表示成功。 */
#define TRIT_OK 0
#define TRIT_ERR_SHORT (-1)  /* 短于头 + 1 字节 */
#define TRIT_ERR_MAGIC (-2)  /* 魔数不是 TRB2 */
#define TRIT_ERR_VERSION (-3)
#define TRIT_ERR_ALGEBRA (-4) /* 语义字节不在 0..=2 */
#define TRIT_ERR_LENGTH (-5)  /* 头声明的长度与实际字节数不符 */
#define TRIT_ERR_BYTE (-6)    /* 出现 243..255（不是合法 5-trit 组合） */
#define TRIT_ERR_CRC (-7)     /* CRC-3 不匹配 */
#define TRIT_ERR_COUNT (-8)   /* count != 3^nvars */
#define TRIT_ERR_CANON (-10) /* 末字节闲置的 trit 槽非零（非规范编码） */
#define TRIT_ERR_NOMEM (-9)

typedef struct {
    int8_t *trits;   /* count 个，base-3 顺序（与 truth.csv 行序一致） */
    uint32_t count;
    uint8_t nvars;
    uint8_t algebra; /* 0=kleene 1=lukasiewicz 2=bochvar */
} trit_table;

/* 解码并校验；成功时 out->trits 由 malloc 分配，需 trit_table_free()。
 * 失败时 out 不被修改。 */
int trit_trb2_decode(const uint8_t *blob, size_t len, trit_table *out);

void trit_table_free(trit_table *table);

#endif
