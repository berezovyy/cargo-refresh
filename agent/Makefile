CC ?= gcc
BUILD ?= build
CPPFLAGS := -Isrc -Isrc/unwind-include
CFLAGS := -O2 -g -Wall -Wextra -Werror
LDLIBS := -Wl,-z,now -Wl,-l:libunwind.so.8 -Wl,-l:libunwind-x86_64.so.8 -ldl -lpthread
SOURCES := $(wildcard src/*.c)
HEADERS := $(wildcard src/*.h)
ASAN_FLAGS := -O1 -g -Wall -Wextra -Werror -fsanitize=address,undefined -fno-omit-frame-pointer

.PHONY: all test test-asan test-equivalence clean
all: $(BUILD)/librefresh.so
$(BUILD):
	mkdir -p $@
$(BUILD)/librefresh.so: $(SOURCES) $(HEADERS) | $(BUILD)
	$(CC) $(CPPFLAGS) $(CFLAGS) -fvisibility=hidden -shared -fPIC $(SOURCES) -o $@ $(LDLIBS)
$(BUILD)/librefresh-testing.so: $(SOURCES) $(HEADERS) | $(BUILD)
	$(CC) $(CPPFLAGS) $(CFLAGS) -DREFRESH_TESTING -fvisibility=hidden -shared -fPIC $(SOURCES) -o $@ $(LDLIBS)
$(BUILD)/librefresh-legacy.so: tests/legacy/agent.c | $(BUILD)
	$(CC) $(CPPFLAGS) $(CFLAGS) -Wno-unused-variable -Itests/legacy -shared -fPIC $< -o $@ $(LDLIBS)
$(BUILD)/host: tests/host.c | $(BUILD)
	$(CC) $(CFLAGS) -fno-pie -no-pie -fno-omit-frame-pointer -rdynamic $< -o $@ -lpthread
$(BUILD)/host-pie: tests/host.c | $(BUILD)
	$(CC) $(CFLAGS) -fPIE -pie -fno-omit-frame-pointer -rdynamic $< -o $@ -lpthread
$(BUILD)/oracle-after: tests/host.c | $(BUILD)
	$(CC) $(CFLAGS) -DF_VALUE=2 -DRO_VALUE=23 -fno-pie -no-pie -fno-omit-frame-pointer -rdynamic $< -o $@ -lpthread
$(BUILD)/test-unwind: tests/test_unwind.c src/unwind-copy.h | $(BUILD)
	$(CC) $(CPPFLAGS) $(CFLAGS) -fno-pie -no-pie $< -o $@
$(BUILD)/test-protocol: tests/test_protocol.c src/protocol.c $(HEADERS) | $(BUILD)
	$(CC) $(CPPFLAGS) $(CFLAGS) -fno-pie -no-pie tests/test_protocol.c src/protocol.c -o $@
test: all $(BUILD)/librefresh-testing.so $(BUILD)/host $(BUILD)/host-pie $(BUILD)/oracle-after $(BUILD)/test-unwind $(BUILD)/test-protocol
	./$(BUILD)/test-unwind
	./$(BUILD)/test-protocol
	REFRESH_TEST_BUILD=$(abspath $(BUILD)) python3 tests/integration.py $(BUILD)/librefresh.so --testing $(BUILD)/librefresh-testing.so
test-equivalence: all $(BUILD)/librefresh-testing.so $(BUILD)/librefresh-legacy.so $(BUILD)/host $(BUILD)/test-unwind
	./$(BUILD)/test-unwind
	REFRESH_TEST_BUILD=$(abspath $(BUILD)) python3 tests/integration.py --equivalence $(BUILD)/librefresh-legacy.so $(BUILD)/librefresh.so $(BUILD)/librefresh-testing.so
test-asan:
	REFRESH_TEST_ASAN_LIBRARY='$(shell $(CC) -print-file-name=libasan.so)' ASAN_OPTIONS=abort_on_error=1:detect_leaks=1 UBSAN_OPTIONS=halt_on_error=1:print_stacktrace=1 $(MAKE) BUILD=build-asan CFLAGS='$(ASAN_FLAGS)' test
clean:
	rm -rf build build-asan
