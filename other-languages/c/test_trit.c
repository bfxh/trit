#include "trit.h"

#include <stdio.h>
#include <string.h>

static int failures;

#define CHECK(cond)                                                            \
  do {                                                                         \
    if (!(cond)) {                                                             \
      fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond);          \
      failures++;                                                              \
    }                                                                          \
  } while (0)

static const unsigned POW3[6] = {1u, 3u, 9u, 27u, 81u, 243u};

static void test_k3_tables(void) {
  static const k3_t not_tbl[3] = {K3_T, K3_U, K3_F};
  static const k3_t and_tbl[3][3] = {
      {K3_F, K3_F, K3_F},
      {K3_F, K3_U, K3_U},
      {K3_F, K3_U, K3_T},
  };
  static const k3_t or_tbl[3][3] = {
      {K3_F, K3_U, K3_T},
      {K3_U, K3_U, K3_T},
      {K3_T, K3_T, K3_T},
  };
  static const k3_t gate_tbl[3][3] = {
      {K3_T, K3_T, K3_T},
      {K3_U, K3_U, K3_U},
      {K3_F, K3_U, K3_T},
  };
  int a;
  int b;
  for (a = 0; a < 3; a++) {
    CHECK(k3_not((k3_t)a) == not_tbl[a]);
    for (b = 0; b < 3; b++) {
      CHECK(k3_and((k3_t)a, (k3_t)b) == and_tbl[a][b]);
      CHECK(k3_or((k3_t)a, (k3_t)b) == or_tbl[a][b]);
      CHECK(k3_gate((k3_t)a, (k3_t)b) == gate_tbl[a][b]);
    }
  }
}

static void test_k3_bitplanes(void) {
  static const uint8_t ones_tbl[3] = {0, 0, 1};
  static const uint8_t unknown_tbl[3] = {0, 1, 0};
  int x;
  for (x = 0; x < 3; x++) {
    uint8_t ones = 9;
    uint8_t unknowns = 9;
    k3_t back = K3_U;
    k3_encode_bits((k3_t)x, &ones, &unknowns);
    CHECK(ones == ones_tbl[x]);
    CHECK(unknowns == unknown_tbl[x]);
    CHECK(k3_decode_bits(ones, unknowns, &back) == TRIT_OK);
    CHECK(back == (k3_t)x);
  }
  {
    k3_t out = K3_U;
    CHECK(k3_decode_bits(1, 1, &out) == TRIT_ERR_INVALID_BITPLANE);
    CHECK(k3_decode_bits(2, 0, &out) == TRIT_ERR_INVALID_BITPLANE);
    CHECK(k3_decode_bits(0, 2, &out) == TRIT_ERR_INVALID_BITPLANE);
  }
}

static void test_pack5_all(void) {
  unsigned v;
  for (v = 0; v < 243u; v++) {
    int8_t trits[5];
    uint8_t byte = 0;
    int8_t back[5];
    int i;
    for (i = 0; i < 5; i++) {
      trits[i] = trit_from_digit((v / POW3[i]) % 3u);
    }
    CHECK(trit_pack5(trits, &byte) == TRIT_OK);
    CHECK(byte == (uint8_t)v);
    CHECK(trit_unpack5(byte, back) == TRIT_OK);
    CHECK(memcmp(trits, back, sizeof(trits)) == 0);
  }
}

static void test_tails(void) {
  size_t n;
  for (n = 1; n <= 5; n++) {
    unsigned v;
    for (v = 0; v < POW3[n]; v++) {
      int8_t trits[5] = {0, 0, 0, 0, 0};
      int8_t full[5] = {0, 0, 0, 0, 0};
      int8_t back[5] = {9, 9, 9, 9, 9};
      uint8_t byte = 0;
      uint8_t full_byte = 0;
      size_t i;
      for (i = 0; i < n; i++) {
        trits[i] = trit_from_digit((v / POW3[i]) % 3u);
        full[i] = trits[i];
      }
      CHECK(trit_pack5_tail(trits, n, &byte) == TRIT_OK);
      CHECK(trit_pack5(full, &full_byte) == TRIT_OK);
      CHECK(byte == full_byte);
      CHECK(trit_unpack5_tail(byte, n, back) == TRIT_OK);
      CHECK(memcmp(trits, back, n) == 0);
    }
  }
}

static void test_errors(void) {
  int8_t bad_pos[5] = {2, 0, 0, 0, 0};
  int8_t bad_neg[5] = {0, 0, 0, 0, -2};
  int8_t out[5];
  uint8_t byte = 0;
  unsigned v;

  CHECK(trit_pack5(bad_pos, &byte) == TRIT_ERR_INVALID_TRIT);
  CHECK(trit_pack5(bad_neg, &byte) == TRIT_ERR_INVALID_TRIT);
  CHECK(trit_pack5_tail(bad_pos, 1, &byte) == TRIT_ERR_INVALID_TRIT);
  CHECK(trit_pack5_tail(bad_neg, 5, &byte) == TRIT_ERR_INVALID_TRIT);
  CHECK(trit_pack5_tail(bad_pos, 0, &byte) == TRIT_ERR_SHORT_INPUT);
  CHECK(trit_pack5_tail(bad_pos, 6, &byte) == TRIT_ERR_SHORT_INPUT);

  for (v = 243u; v <= 255u; v++) {
    CHECK(trit_unpack5((uint8_t)v, out) == TRIT_ERR_INVALID_BYTE);
    CHECK(trit_unpack5_tail((uint8_t)v, 5, out) == TRIT_ERR_INVALID_BYTE);
  }
  CHECK(trit_unpack5_tail(0, 6, out) == TRIT_ERR_SHORT_INPUT);
}

int main(void) {
  test_k3_tables();
  test_k3_bitplanes();
  test_pack5_all();
  test_tails();
  test_errors();
  if (failures != 0) {
    fprintf(stderr, "test_trit: %d failure(s)\n", failures);
    return 1;
  }
  puts("test_trit: all pass");
  return 0;
}
