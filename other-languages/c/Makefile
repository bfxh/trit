CC ?= cc
CFLAGS ?= -std=c99 -O2 -Wall -Wextra -Werror -pedantic
BUILD := build

all: $(BUILD)/test_trit $(BUILD)/trit_conformance

$(BUILD):
	mkdir -p $(BUILD)

$(BUILD)/test_trit: test_trit.c trit.h | $(BUILD)
	$(CC) $(CFLAGS) -o $@ test_trit.c

$(BUILD)/trit_conformance: conformance.c trit.h | $(BUILD)
	$(CC) $(CFLAGS) -o $@ conformance.c

test: $(BUILD)/test_trit
	./$(BUILD)/test_trit

conformance: $(BUILD)/trit_conformance
	./$(BUILD)/trit_conformance > $(BUILD)/c-vectors.jsonl

clean:
	rm -rf $(BUILD)

.PHONY: all test conformance clean
