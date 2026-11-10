/*
 * conformance.c - emit JSONL for the interop v0 core profiles.
 *
 * The Rust runner compares this output with the golden vectors; this program
 * does not read the vectors. Core profiles: k3 (28), kgate (9), pack5 (657).
 */
#include "trit.h"

#include <stdio.h>

static const unsigned POW3[6] = {1u, 3u, 9u, 27u, 81u, 243u};
static const k3_t K3_VALUES[3] = {K3_F, K3_U, K3_T};
static const char *const K3_IDS[3] = {"f", "u", "t"};
static const unsigned TAIL_BYTES[9] = {0u, 1u, 2u, 3u, 40u, 80u, 121u, 122u,
                                       242u};

static void emit_str(const char *id, const char *value) {
  printf("{\"id\":\"%s\",\"expected\":\"%s\"}\n", id, value);
}

static void emit_nat(const char *id, unsigned value) {
  printf("{\"id\":\"%s\",\"expected\":%u}\n", id, value);
}

static void emit_bits(const char *id, uint8_t ones, uint8_t unknowns) {
  printf("{\"id\":\"%s\",\"expected\":[%u,%u]}\n", id, (unsigned)ones,
         (unsigned)unknowns);
}

static void emit_trits(const char *id, const int8_t *trits, size_t count) {
  size_t i;
  printf("{\"id\":\"%s\",\"expected\":[", id);
  for (i = 0; i < count; i++) {
    printf("%s%d", i == 0 ? "" : ",", (int)trits[i]);
  }
  printf("]}\n");
}

static void emit_error(const char *id, const char *error) {
  printf("{\"id\":\"%s\",\"error\":\"%s\"}\n", id, error);
}

static void emit_k3(void) {
  char id[64];
  int a;
  int b;
  for (a = 0; a < 3; a++) {
    snprintf(id, sizeof(id), "k3.not.%s", K3_IDS[a]);
    emit_str(id, k3_name(k3_not(K3_VALUES[a])));
  }
  for (a = 0; a < 3; a++) {
    for (b = 0; b < 3; b++) {
      snprintf(id, sizeof(id), "k3.and.%s.%s", K3_IDS[a], K3_IDS[b]);
      emit_str(id, k3_name(k3_and(K3_VALUES[a], K3_VALUES[b])));
      snprintf(id, sizeof(id), "k3.or.%s.%s", K3_IDS[a], K3_IDS[b]);
      emit_str(id, k3_name(k3_or(K3_VALUES[a], K3_VALUES[b])));
    }
  }
  for (a = 0; a < 3; a++) {
    uint8_t ones = 0;
    uint8_t unknowns = 0;
    k3_encode_bits(K3_VALUES[a], &ones, &unknowns);
    snprintf(id, sizeof(id), "k3.bitplane.encode.%s", K3_IDS[a]);
    emit_bits(id, ones, unknowns);
  }
  emit_str("k3.bitplane.decode.00", "F");
  emit_str("k3.bitplane.decode.10", "T");
  emit_str("k3.bitplane.decode.01", "U");
  emit_error("k3.bitplane.decode.invalid.11", "invalid_bitplane");

  for (a = 0; a < 3; a++) {
    for (b = 0; b < 3; b++) {
      snprintf(id, sizeof(id), "kgate.%s.%s", K3_IDS[a], K3_IDS[b]);
      emit_str(id, k3_name(k3_gate(K3_VALUES[a], K3_VALUES[b])));
    }
  }
}

static void emit_pack5_full(void) {
  int d0;
  int d1;
  int d2;
  int d3;
  int d4;
  for (d0 = 0; d0 < 3; d0++) {
    for (d1 = 0; d1 < 3; d1++) {
      for (d2 = 0; d2 < 3; d2++) {
        for (d3 = 0; d3 < 3; d3++) {
          for (d4 = 0; d4 < 3; d4++) {
            int8_t trits[5] = {(int8_t)(d0 - 1), (int8_t)(d1 - 1),
                               (int8_t)(d2 - 1), (int8_t)(d3 - 1),
                               (int8_t)(d4 - 1)};
            unsigned value = (unsigned)d0 + 3u * (unsigned)d1 +
                             9u * (unsigned)d2 + 27u * (unsigned)d3 +
                             81u * (unsigned)d4;
            uint8_t byte = 0;
            char id[32];
            trit_error err = trit_pack5(trits, &byte);
            snprintf(id, sizeof(id), "pack5.pack.%03u", value);
            if (err == TRIT_OK) {
              emit_nat(id, (unsigned)byte);
            } else {
              emit_error(id, trit_error_name(err));
            }
          }
        }
      }
    }
  }
}

static void emit_pack5_tails(void) {
  unsigned n;
  for (n = 1; n <= 4; n++) {
    unsigned combo;
    for (combo = 0; combo < POW3[n]; combo++) {
      int8_t trits[4];
      unsigned value = 0;
      unsigned i;
      uint8_t byte = 0;
      char id[48];
      trit_error err;
      for (i = 0; i < n; i++) {
        unsigned digit = (combo / POW3[i]) % 3u;
        trits[i] = trit_from_digit(digit);
        value += digit * POW3[i];
      }
      for (i = n; i < 5; i++) {
        value += POW3[i];
      }
      err = trit_pack5_tail(trits, (size_t)n, &byte);
      snprintf(id, sizeof(id), "pack5.pack.tail.%u.%03u", n, value);
      if (err == TRIT_OK) {
        emit_nat(id, (unsigned)byte);
      } else {
        emit_error(id, trit_error_name(err));
      }
    }
  }
}

static void emit_unpack5_full(void) {
  unsigned value;
  for (value = 0; value < 243u; value++) {
    int8_t trits[5];
    char id[32];
    trit_error err = trit_unpack5((uint8_t)value, trits);
    snprintf(id, sizeof(id), "pack5.unpack.%03u", value);
    if (err == TRIT_OK) {
      emit_trits(id, trits, 5);
    } else {
      emit_error(id, trit_error_name(err));
    }
  }
}

static void emit_unpack5_tails(void) {
  unsigned n;
  size_t b;
  for (n = 1; n <= 4; n++) {
    for (b = 0; b < sizeof(TAIL_BYTES) / sizeof(TAIL_BYTES[0]); b++) {
      unsigned byte = TAIL_BYTES[b];
      int8_t trits[5];
      char id[48];
      trit_error err = trit_unpack5_tail((uint8_t)byte, (size_t)n, trits);
      snprintf(id, sizeof(id), "pack5.unpack.tail.%u.%03u", n, byte);
      if (err == TRIT_OK) {
        emit_trits(id, trits, n);
      } else {
        emit_error(id, trit_error_name(err));
      }
    }
  }
}

static void emit_errors(void) {
  unsigned value;
  for (value = 243u; value <= 255u; value++) {
    int8_t trits[5];
    char id[40];
    trit_error err = trit_unpack5((uint8_t)value, trits);
    snprintf(id, sizeof(id), "pack5.unpack.invalid.%03u", value);
    emit_error(id, trit_error_name(err));
  }
  {
    int8_t trits[5];
    trit_error err = trit_unpack5_tail(0, 6, trits);
    emit_error("pack5.unpack.short_input.006.001", trit_error_name(err));
  }
  {
    int8_t bad[5] = {2, 0, 0, 0, 0};
    uint8_t byte = 0;
    trit_error err = trit_pack5(bad, &byte);
    emit_error("pack5.pack.invalid_trit.2", trit_error_name(err));
  }
}

int main(void) {
  emit_k3();
  emit_pack5_full();
  emit_pack5_tails();
  emit_unpack5_full();
  emit_unpack5_tails();
  emit_errors();
  return 0;
}
