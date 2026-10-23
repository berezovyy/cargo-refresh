CC ?= gcc
CPPFLAGS := -Isrc -Isrc/unwind-include
CFLAGS := -O2 -g -Wall -Wextra -Werror
LDLIBS := -Wl,-z,now -Wl,-l:libunwind.so.8 -Wl,-l:libunwind-x86_64.so.8 -ldl -lpthread
SOURCES := $(wildcard src/*.c)
HEADERS := $(wildcard src/*.h)

.PHONY: all test test-asan clean
all: build/librefresh.so
build:
	mkdir -p $@
build/librefresh.so: $(SOURCES) $(HEADERS) | build
	$(CC) $(CPPFLAGS) $(CFLAGS) -shared -fPIC $(SOURCES) -o $@ $(LDLIBS)
build/librefresh-testing.so: $(SOURCES) $(HEADERS) | build
	$(CC) $(CPPFLAGS) $(CFLAGS) -DREFRESH_TESTING -shared -fPIC $(SOURCES) -o $@ $(LDLIBS)
build/librefresh-legacy.so: tests/legacy/agent.c | build
	$(CC) $(CPPFLAGS) $(CFLAGS) -Wno-unused-variable -Itests/legacy -shared -fPIC $< -o $@ $(LDLIBS)
build/host: tests/host.c | build
	$(CC) $(CFLAGS) -fno-pie -no-pie -fno-omit-frame-pointer -rdynamic $< -o $@ -lpthread
build/test-unwind: tests/test_unwind.c src/unwind-copy.h | build
	$(CC) $(CPPFLAGS) $(CFLAGS) $< -o $@
test: all build/host build/test-unwind
	./build/test-unwind
	python3 tests/integration.py build/librefresh.so
test-asan:
	$(MAKE) clean
	$(MAKE) CFLAGS='-O1 -g -Wall -Wextra -Werror -fsanitize=address,undefined -fno-omit-frame-pointer' test
clean:
	rm -rf build

.PHONY: test-equivalence
test-equivalence: all build/librefresh-testing.so build/librefresh-legacy.so build/host build/test-unwind
	./build/test-unwind
	python3 tests/integration.py build/librefresh-legacy.so build/librefresh.so build/librefresh-testing.so
