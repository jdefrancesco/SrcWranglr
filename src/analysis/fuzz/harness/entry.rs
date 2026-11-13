//! The function each fuzzer calls with an input, around the statements
//! that run it: libFuzzer's `LLVMFuzzerTestOneInput`, or for AFL++ a
//! `main` that runs inputs in persistent mode.

use super::Engine;

/// What runs one input in an AFL++ harness.
pub(super) const AFL_ENTRY: &str = "fuzz_one_input";

/// How many inputs one AFL++ process runs before afl-fuzz starts afresh.
pub(super) const AFL_LOOP_COUNT: u32 = 10_000;

/// The standard headers an engine's entry needs.
pub(super) fn includes(engine: Engine) -> &'static [&'static str] {
    match engine {
        Engine::LibFuzzer => &[],
        Engine::Afl => &["<cstdio>", "<vector>"],
    }
}

/// `statements` (reading `data` and `size`) in the engine's entry.
pub(super) fn wrapped(statements: &str, engine: Engine) -> String {
    let signature = "(const uint8_t *data, size_t size)";
    match engine {
        Engine::LibFuzzer => {
            format!("extern \"C\" int LLVMFuzzerTestOneInput{signature} {{\n{statements}\n}}")
        }
        Engine::Afl => format!(
            "static int {AFL_ENTRY}{signature} {{\n{statements}\n}}\n\n{}",
            afl_main()
        ),
    }
}

/// AFL++'s persistent mode: each input arrives in memory shared with
/// afl-fuzz, and one process runs many. Built without AFL++ (plain
/// clang++), it runs one input read from stdin instead.
fn afl_main() -> String {
    format!(
        "#ifdef __AFL_FUZZ_TESTCASE_LEN
#include <unistd.h>  // AFL++'s __AFL_FUZZ_TESTCASE_LEN calls read().

__AFL_FUZZ_INIT();

int main() {{
#ifdef __AFL_HAVE_MANUAL_CONTROL
  // The fork server starts here, after any setup above it.
  __AFL_INIT();
#endif
  // Each input, in memory shared with afl-fuzz: no file to read.
  unsigned char *buf = __AFL_FUZZ_TESTCASE_BUF;
  while (__AFL_LOOP({AFL_LOOP_COUNT})) {{
    {AFL_ENTRY}(buf, __AFL_FUZZ_TESTCASE_LEN);
  }}
  return 0;
}}
#else
// Built without AFL++: runs one input from stdin, to replay a crash.
int main() {{
  std::vector<uint8_t> input;
  uint8_t chunk[4096];
  size_t got;
  while ((got = std::fread(chunk, 1, sizeof chunk, stdin)) > 0) {{
    input.insert(input.end(), chunk, chunk + got);
  }}
  return {AFL_ENTRY}(input.data(), input.size());
}}
#endif"
    )
}
