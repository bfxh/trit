/*
 * trit.h - header-only C99 reference for the interop v0 core profiles.
 *
 * Scope: K3 (NOT/AND/OR + bitplane), KGate, pack5 (5 trits/byte, tails,
 * structured errors). No allocation, no global state, no external deps.
 * Semantics follow interop/spec/v0.md; this header makes no performance claim.
 */
#ifndef TRIT_H
#define TRIT_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef enum {
  TRIT_OK = 0,
  TRIT_ERR_INVALID_BITPLANE = 1,
  TRIT_ERR_INVALID_BYTE = 2,
  TRIT_ERR_SHORT_INPUT = 3,
  TRIT_ERR_INVALID_TRIT = 4
} trit_error;

/* Values are stable: vectors use the strings, code may use the enum. */
typedef enum { K3_F = 0, K3_U = 1, K3_T = 2 } k3_t;

static inline const char *trit_error_name(trit_error err) {
  switch (err) {
  case TRIT_OK:
    return "ok";
  case TRIT_ERR_INVALID_BITPLANE:
    return "invalid_bitplane";
  case TRIT_ERR_INVALID_BYTE:
    return "invalid_byte";
  case TRIT_ERR_SHORT_INPUT:
    return "short_input";
  case TRIT_ERR_INVALID_TRIT:
    return "invalid_trit";
  }
  return "unknown";
}

static inline const char *k3_name(k3_t x) {
  switch (x) {
  case K3_F:
    return "F";
  case K3_U:
    return "U";
  case K3_T:
    return "T";
  }
  return "?";
}

static inline k3_t k3_not(k3_t x) {
  if (x == K3_F) {
    return K3_T;
  }
  if (x == K3_T) {
    return K3_F;
  }
  return K3_U;
}

static inline k3_t k3_and(k3_t a, k3_t b) {
  if (a == K3_F || b == K3_F) {
    return K3_F;
  }
  if (a == K3_T && b == K3_T) {
    return K3_T;
  }
  return K3_U;
}

static inline k3_t k3_or(k3_t a, k3_t b) {
  if (a == K3_T || b == K3_T) {
    return K3_T;
  }
  if (a == K3_F && b == K3_F) {
    return K3_F;
  }
  return K3_U;
}

/* Admission gate: required=F -> T, required=U -> U, required=T -> verified. */
static inline k3_t k3_gate(k3_t required, k3_t verified) {
  if (required == K3_F) {
    return K3_T;
  }
  if (required == K3_U) {
    return K3_U;
  }
  return verified;
}

/* Bitplanes: F=(0,0), T=(1,0), U=(0,1); (1,1) is invalid in K3. */
static inline void k3_encode_bits(k3_t x, uint8_t *ones, uint8_t *unknowns) {
  uint8_t o = 0;
  uint8_t u = 0;
  if (x == K3_T) {
    o = 1;
  } else if (x == K3_U) {
    u = 1;
  }
  *ones = o;
  *unknowns = u;
}

static inline trit_error k3_decode_bits(uint8_t ones, uint8_t unknowns,
                                        k3_t *out) {
  if (ones > 1 || unknowns > 1 || (ones == 1 && unknowns == 1)) {
    return TRIT_ERR_INVALID_BITPLANE;
  }
  *out = ones == 1 ? K3_T : unknowns == 1 ? K3_U : K3_F;
  return TRIT_OK;
}

#define TRIT_PACK5_LIMIT 243u

/* Balanced trit -> digit: -1->0, 0->1, +1->2. */
static inline uint8_t trit_digit(int8_t t) { return (uint8_t)(t + 1); }

static inline int8_t trit_from_digit(unsigned d) { return (int8_t)d - 1; }

static inline int trit_is_valid(int8_t t) { return t >= -1 && t <= 1; }

static inline trit_error trit_pack5(const int8_t trits[5], uint8_t *out) {
  static const unsigned weights[5] = {1u, 3u, 9u, 27u, 81u};
  unsigned value = 0;
  size_t i;
  for (i = 0; i < 5; i++) {
    if (!trit_is_valid(trits[i])) {
      return TRIT_ERR_INVALID_TRIT;
    }
    value += (unsigned)trit_digit(trits[i]) * weights[i];
  }
  *out = (uint8_t)value;
  return TRIT_OK;
}

/* Tail variant: count 1..5, missing positions padded with trit 0. */
static inline trit_error trit_pack5_tail(const int8_t *trits, size_t count,
                                        uint8_t *out) {
  int8_t padded[5] = {0, 0, 0, 0, 0};
  size_t i;
  if (count < 1 || count > 5) {
    return TRIT_ERR_SHORT_INPUT;
  }
  for (i = 0; i < count; i++) {
    if (!trit_is_valid(trits[i])) {
      return TRIT_ERR_INVALID_TRIT;
    }
    padded[i] = trits[i];
  }
  return trit_pack5(padded, out);
}

static inline trit_error trit_unpack5(uint8_t byte, int8_t out[5]) {
  static const unsigned weights[5] = {1u, 3u, 9u, 27u, 81u};
  size_t i;
  if (byte >= TRIT_PACK5_LIMIT) {
    return TRIT_ERR_INVALID_BYTE;
  }
  for (i = 0; i < 5; i++) {
    out[i] = trit_from_digit(((unsigned)byte / weights[i]) % 3u);
  }
  return TRIT_OK;
}

/* Tail variant: count 0..5, output receives the first count trits. */
static inline trit_error trit_unpack5_tail(uint8_t byte, size_t count,
                                          int8_t *out) {
  int8_t all[5];
  size_t i;
  trit_error err;
  if (byte >= TRIT_PACK5_LIMIT) {
    return TRIT_ERR_INVALID_BYTE;
  }
  if (count > 5) {
    return TRIT_ERR_SHORT_INPUT;
  }
  err = trit_unpack5(byte, all);
  if (err != TRIT_OK) {
    return err;
  }
  for (i = 0; i < count; i++) {
    out[i] = all[i];
  }
  return TRIT_OK;
}

#ifdef __cplusplus
}
#endif

#endif /* TRIT_H */
