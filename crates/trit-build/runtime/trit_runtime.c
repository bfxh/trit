#include "trit_runtime.h"

#include <stdlib.h>
#include <string.h>

#define HEADER 11u
#define VERSION 2u
#define MAGIC "TRB2"

static uint8_t crc_coefficients[4] = {1, 0, 2, 1}; /* x^3 + 2x + 1 */

static uint32_t pow3(uint8_t nvars) {
    uint32_t value = 1;
    for (uint8_t i = 0; i < nvars; i++) {
        value *= 3u;
    }
    return value;
}

/* 逐 trit 的 GF(3) CRC-3，与 Rust `trb::crc3` 同构。 */
static int crc3(const uint8_t *digits, uint32_t len, uint8_t out[3]) {
    size_t total = (size_t)len + 3u;
    uint8_t *buf = malloc(total);
    if (buf == NULL) {
        return TRIT_ERR_NOMEM;
    }
    memcpy(buf, digits, len);
    buf[len] = 0;
    buf[len + 1] = 0;
    buf[len + 2] = 0;
    for (size_t i = 0; i < total - 3u; i++) {
        uint8_t lead = buf[i] % 3u;
        if (lead == 0) {
            continue;
        }
        for (size_t k = 0; k < 4; k++) {
            buf[i + k] = (uint8_t)((buf[i + k] + 3u - (lead * crc_coefficients[k]) % 3u) % 3u);
        }
    }
    out[0] = buf[total - 3u];
    out[1] = buf[total - 2u];
    out[2] = buf[total - 1u];
    free(buf);
    return TRIT_OK;
}

int trit_trb2_decode(const uint8_t *blob, size_t len, trit_table *out) {
    if (blob == NULL || out == NULL) {
        return TRIT_ERR_NOMEM;
    }
    if (len < HEADER + 1u) {
        return TRIT_ERR_SHORT;
    }
    if (memcmp(blob, MAGIC, 4) != 0) {
        return TRIT_ERR_MAGIC;
    }
    if (blob[4] != VERSION) {
        return TRIT_ERR_VERSION;
    }
    if (blob[5] > 2u) {
        return TRIT_ERR_ALGEBRA;
    }
    uint8_t nvars = blob[6];
    if (nvars > 20u) { /* 3^20 已超 u32，避免 pow3 回绕 */
        return TRIT_ERR_COUNT;
    }
    uint32_t count = (uint32_t)blob[7] | ((uint32_t)blob[8] << 8) |
                     ((uint32_t)blob[9] << 16) | ((uint32_t)blob[10] << 24);
    uint32_t payload = count + 3u;
    size_t expected = HEADER + ((size_t)payload + 4u) / 5u;
    if (len != expected) {
        return TRIT_ERR_LENGTH;
    }
    if (count != pow3(nvars)) {
        return TRIT_ERR_COUNT;
    }

    uint8_t *digits = malloc(payload);
    if (digits == NULL) {
        return TRIT_ERR_NOMEM;
    }
    size_t filled = 0;
    for (size_t i = HEADER; i < len && filled < payload; i++) {
        uint32_t byte = blob[i];
        if (byte >= 243u) {
            free(digits);
            return TRIT_ERR_BYTE;
        }
        for (int slot = 0; slot < 5 && filled < payload; slot++) {
            digits[filled++] = (uint8_t)(byte % 3u);
            byte /= 3u;
        }
    }
    if (filled != payload) {
        free(digits);
        return TRIT_ERR_LENGTH;
    }

    uint8_t checksum[3];
    int status = crc3(digits, count, checksum);
    if (status != TRIT_OK) {
        free(digits);
        return status;
    }
    if (checksum[0] != digits[count] || checksum[1] != digits[count + 1] ||
        checksum[2] != digits[count + 2]) {
        free(digits);
        return TRIT_ERR_CRC;
    }

    int8_t *trits = malloc(count == 0 ? 1u : count);
    if (trits == NULL) {
        free(digits);
        return TRIT_ERR_NOMEM;
    }
    for (uint32_t i = 0; i < count; i++) {
        trits[i] = (int8_t)digits[i] - 1; /* 0/1/2 -> -1/0/1 */
    }
    free(digits);

    out->trits = trits;
    out->count = count;
    out->nvars = nvars;
    out->algebra = blob[5];
    return TRIT_OK;
}

void trit_table_free(trit_table *table) {
    if (table != NULL) {
        free(table->trits);
        table->trits = NULL;
        table->count = 0;
    }
}
