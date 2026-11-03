# Tests du cœur portable du pilote (gcc ou clang).
#   make test      : compile avec avertissements stricts + sanitizers, exécute
#   make coverage  : mesure la couverture (gcov), échoue sous 100 % de lignes
CC      ?= cc
CFLAGS  := -std=c99 -O1 -g -Wall -Wextra -Werror -pedantic -Wconversion \
           -Wshadow -Wcast-qual -Wstrict-prototypes -Wmissing-prototypes
SAN     := -fsanitize=address,undefined -fno-sanitize-recover=all
SRC     := ../core/ms_loopback.c test_ms_loopback.c
BUILD   := build

.PHONY: test coverage clean
test: $(BUILD)/test_ms_loopback
	./$(BUILD)/test_ms_loopback

$(BUILD)/test_ms_loopback: $(SRC) ../core/ms_loopback.h
	mkdir -p $(BUILD)
	$(CC) $(CFLAGS) $(SAN) $(SRC) -o $@

coverage:
	mkdir -p $(BUILD)/cov
	$(CC) $(CFLAGS) --coverage $(SRC) -o $(BUILD)/cov/test_cov
	cd $(BUILD)/cov && ./test_cov && gcov -o . test_cov-ms_loopback.gcda | tee gcov.txt
	grep -A1 "ms_loopback.c'" $(BUILD)/cov/gcov.txt | grep -q "Lines executed:100.00%" \
	  || (echo "Couverture < 100 %"; exit 1)

clean:
	rm -rf $(BUILD)
