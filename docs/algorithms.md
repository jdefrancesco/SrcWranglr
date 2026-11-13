# Algorithms

How SrcWranglr's analysis works, for anyone reading or changing it: what each feature computes, which standard technique it uses (where one applies), what it bounds, and where it gives up. For using the features, see the [user guide](guide.md). Two areas are covered in more depth in [How semantic matching and source → sink tracing work](matching-and-tracing.md).

Everything here works offline from the text of the source. SrcWranglr parses C and C++ with [tree-sitter](https://tree-sitter.github.io/) (the C++ grammar, for C too). It never compiles the project, runs its code or build scripts, or uses a language model. So it knows syntax and names, not types, macros' expansions, or what a pointer points to. Its answers are leads: a path it shows *may* exist, and one it doesn't show may still exist.

The audited project is treated as hostile throughout: every walk has a limit, files are read without following symlinks, and text from the project is shown with hidden characters marked.

## At a glance

| Feature | Technique | Code |
|---|---|---|
| Finding files | gitignore-aware directory walk, sorted, capped | `src/index/discover.rs` |
| Declarations and references | tree-sitter syntax walk with a scope stack | `src/index/extract.rs` |
| Name resolution | lexical scope chain, BFS over base classes, unique-name fallback, memoized | `src/index/resolve.rs` |
| Line → function | sweep line with a stack of open intervals | `src/index/placement.rs` |
| Complexity | McCabe cyclomatic complexity | `src/index/complexity.rs` |
| Trail view | three-column layout ranked by edge kind | `src/graph/trail.rs` |
| Call hierarchy | BFS spanning tree, tidy-tree placement, entry points ranked by BFS reach | `src/graph/hierarchy/` |
| Risky calls | name tables plus argument heuristics (format strings, sizes, flags) | `src/analysis/risky/`, `heuristics.rs` |
| Input reachability | multi-source BFS over the call graph, shortest paths | `src/analysis/reach/` |
| Trace Back | depth-first search over syntactic reaching definitions, call-string context | `src/analysis/trace/walk.rs`, `defs.rs` |
| Trace Where It Goes / source → sink | forward def-use following, interprocedural with a return stack | `src/analysis/trace/forward/` |
| Fuzz targets | additive scoring with reasons; harness drafts for libFuzzer and AFL++; dictionaries of compared constants; Makefiles from the files a function needs | `src/analysis/fuzz/` |
| Look-alike code | token-shape normalization, hashing, polynomial rolling hash | `src/analysis/lookalike/` |
| Code like a function | 5-token shingles, bottom-k MinHash sketch, Jaccard estimate | `src/analysis/lookalike/tokens.rs` |
| Behavior tags | call classification, BFS a few calls deep | `src/analysis/behavior.rs` |
| Rules from code | metavariable abstraction; anti-unification with LCS statement alignment | `src/analysis/similar/` |
| Wrappers of risky calls | parameter pass-through matching | `src/analysis/wrappers.rs` |
| Git history | keyword classification, commit walk by time, parallel blame in a sandbox process | `src/history/` |
| SARIF import | path containment, UTF-16 column conversion | `src/analysis/sarif.rs` |
| Binary survey | magic-byte sniffing, ELF/PE/Mach-O header and symbol checks | `src/app/survey*`, `goblin` |

---

## 1. Indexing

### Finding the files

`discover_sources` walks the project with the `ignore` crate (ripgrep's walker):
- `.gitignore` and `.ignore` apply even outside a git checkout;
- hidden files and folders are skipped;
- symlinks aren't followed;
- `node_modules` is skipped anywhere, and `target` only when a `Cargo.toml` sits beside it (so the Linux kernel's `drivers/target` stays).

C and C++ files are picked by extension, case-insensitively: `c cc cpp cxx c++ h hh hpp hxx h++ inl ipp tpp`. Paths are sorted and cut to 100,000 files; files over 2 MiB are skipped with a warning. Each file is read with its size checked first and decoded as lossy UTF-8.

### Extracting declarations and references

`Extractor` walks each file's syntax tree once, keeping a stack of the scopes it is in (namespaces, classes):
- **Declarations:** namespaces, classes, structs, unions, functions, methods, fields and `typedef struct` names, including those in `#if`/`#ifdef` branches (all branches are read, since nothing is preprocessed). Anonymous namespaces and structs fold into the scope around them. A prototype is a declaration; a body makes it a definition.
- **References:** calls (`Calls`), base classes (`Inherits`), and identifiers, field initializers and type names (`Uses`). Function bodies are walked iteratively with an explicit stack, so deep nesting can't overflow it.
- **Locals:** each parameter and local is mapped to its declared type, so a local is never mistaken for a global of the same name, and `x.m` on a typed local becomes `Type::m`. `this->m` is looked up in the class; a member of something untyped is looked up by unique name for calls and dropped for fields (too ambiguous).
- **Free calls** keep their argument text (up to 8 arguments, 1,024 characters each), which the argument heuristics read later.

The indexer runs on one background thread with a 256 MiB stack (tree-sitter recurses on long cast chains), checks a cancel flag between files, and turns a panic into an error instead of a crash.

### Resolving names

`build_graph` (`resolve.rs`) first merges declarations by qualified name: a definition wins over a declaration, and among equals the first file. So an in-class prototype and its out-of-line definition become one symbol, and **overloads become one symbol** too. Symbol ids are given in sorted order, so they are the same from run to run.

Each reference is then resolved:
1. **Scope chain:** from the innermost scope out (`a::B::x`, then `a::x`, then `x`). At a class scope, its base classes are searched too, by a breadth-first search with a visited set (inheritance cycles can't loop). Then the global scope. A member written through a derived class (`Derived::m`) is found on its base.
2. **Unique-name fallback**, for calls and types only: if exactly one symbol's qualified name is the name or ends in `::name`, it is the one. **Ambiguous names stay unlinked** rather than guessed.

Results are memoized per (scope, name, lookup kind). Edges are deduplicated, and self-loops and a member's reference to its own class are dropped.

A call that resolves to nothing in the project is an **external call** (`memcpy`, `recv`): kept with its caller and argument text. A call that resolves to the project's own *global* function with a name the risky-call rules watch (the Linux kernel's `strcpy` or `copy_from_user`) is kept as a **library call in tree**, so it is judged like the library's.

### Smaller passes

- **Line → function** (`placement.rs`): which function holds each of many lines, found by a sweep line. Functions sorted by start and lines by number, a stack of open functions pushed and popped as the sweep passes them: O((F + L) log).
- **Complexity** (`complexity.rs`): McCabe cyclomatic complexity, 1 plus one per `if`, `for`, range-`for`, `while`, `do`, `catch`, `?:`, `case` with a value, `&&` and `||`. Lambdas count toward the function around them.
- **Signatures** (`signature.rs`): the prototype from the definition's start to its declarator's end, comments removed and whitespace collapsed; parameters, `static`, variadic and template read from it.
- **Strings** (`strings.rs`, `macro_strings.rs`): string literals from the syntax tree, and from macro bodies (plain text to tree-sitter) by a small lexer that skips comments and char literals and knows escapes, prefixes and raw strings.
- **Comment marks** (`comment_marks.rs`): TODO-style tags (capitalized whole words), and comments that silence analyzers (`nosemgrep`, `NOLINT`, `cppcheck-suppress`, `coverity[`, `lgtm[`/`codeql[`, `NOSONAR`, `//-V`, …), found with a cheap substring prefilter before the exact checks.

## 2. Graph layout

### Trail

`build_trail` lays out three columns around the selected symbol:
- **Center:** the symbol, or its class's box listing it and the members directly related to it. A class shows up to 24 members; the rest are counted.
- **Sides:** every symbol linked to it, except containment. Ranked by edge kind (inherits, then calls, then uses) and name; up to 40 are shown and the rest counted. What it uses or calls goes right, what uses or calls it goes left. Neighbors are grouped into their class's box.
- **Members** are ordered functions, fields, then nested types, each by source line.

Columns are stacked vertically around the center and spaced apart horizontally. Edges are drawn as cubic Bézier curves between facing pill edges; edges within one column loop out to the side by an amount that grows with their height, so stacked loops don't overlap.

### Call hierarchy

- **Callees in source order:** each function's callees are ordered by where each is first called. Direct recursion is marked ↺.
- **Entry points:** functions that call something and that nothing calls (or, in pure cycles, all callers). Ranked by how many functions a breadth-first search reaches from each (counted up to 300), with `main` first on ties. Known program entries (`main`, `WinMain`, `DllMain`, `LLVMFuzzerTestOneInput`, `_start`, …) are listed first.
- **The tree** is a breadth-first spanning tree: each function appears once, at its shallowest depth, under the first caller to reach it, so tree edges never cross. Other calls among shown functions are drawn only on hover. Depth, children and nodes are capped (6 / 12 / 200 when focused), with "+N" for what is left out.
- **Placement** is a tidy tree: one column per depth, each subtree its own band of rows, a parent centered on its children.

The **overview** is one box per file listing its top-level symbols, laid out in a grid of ⌈√n⌉ columns.

## 3. Judging risky calls

Each external call and library call in tree is judged by `risky::library_call`:

1. **Your dangerous-functions list** first (`~/.config/srcwranglr/dangerous.toml`, and the project's once trusted). An untrusted project's list may only add or raise findings.
2. **Argument checks** (`heuristics::assess`) read the call's argument text and return *flag* (with a specific finding), *suppress* (it's fine), or *default*:
   - **Format strings** (printf and scanf families, including `printk`, `syslog`, `wsprintf`): a format that isn't a string literal is flagged (CWE-134). A literal joined to capitalized macros (`KERN_INFO "…"`) counts as literal. The `v` forms (`vprintf`) aren't flagged, since they pass on their caller's format. For `sprintf` into an unsized buffer, the format's conversions are parsed: an unbounded `%s` is high, a long numeric conversion medium.
   - **scanf:** a `%s` or `%[` without a width is flagged; a format with no string conversions is suppressed.
   - **Copies** (`memcpy`, `strncpy`, …): a size taken from the source (`strlen(src)`) is flagged; a constant-size memory copy is flagged low.
   - **Allocations** (`malloc`, `kmalloc`, `HeapAlloc`, …): a size that is a product of two or more factors, not all constants, can overflow (CWE-190).
   - **Others:** a string copy from a literal, a command from a literal, `alloca` sizes, `realpath(p, NULL)` (suppressed: it allocates), and certificate checks turned off (`SSL_VERIFY_NONE`, `CURLOPT_SSL_VERIFYPEER` set to `0`, `0L`, `(long)0` or `FALSE`).
3. **Default:** the function's rule from the built-in tables (about 300 names in C library, Windows, Linux kernel and weak-crypto sets), with severity, CWE, why and fix.

A project's own definition of a watched name is judged the same way, except for generic names (`access`, `random`, `warn`) and crypto names, which in a project are more often its own unrelated function or an implementation.

## 4. Input reachability

Which findings outside input may get to (`src/analysis/reach/`).

**Sources.** Library calls that read input, each with where the input lands:
- an argument: `recv`, `read`, `fread`, `fgets`, `getline`, `ReadFile`, `SSL_read`, …;
- the arguments from some position on: `scanf`, `fscanf`;
- the return value: `getc`, `getchar`, `getenv`.

`main`'s pointer and array parameters count as the command line. Only unqualified, `std::` or global names count, so a project's `mylib::read` doesn't. A function making such a call is a **reader**; the variables the input lands in are its **input names**.

**First step: only the calls handed the input.** Inside a reader, a callee counts only if one of its arguments mentions an input name, or calls a source directly (`parse(getenv(…))`). This is word matching on the code with literals and comments blanked, one step deep, not data flow; it stops a reader's unrelated calls (its logging, say) from all counting as reached.

**Propagation:** a multi-source **breadth-first search** over direct call edges, seeded with every reader's handed callees. Each function's predecessor is set the first time it is reached. Breadth-first order makes each kept path a **shortest path** (fewest calls) and ends cycles. Callees are visited in name order, so results don't vary between runs. A reader that other functions also call has all its callees counted.

**A finding is reachable** if its function was reached through a caller, or if the function is a reader and the flagged call (the whole call at the finding's column, even across lines) uses an input name. The path shown, "`recv()` in serve » handle_packet » parse_header", follows the predecessors back to the reader.

**What it misses:** calls through function pointers and virtual dispatch, and input passed through globals. So "not reachable" means only that no path was found.

## 5. Tracing a value back

**Trace Back** (`src/analysis/trace/walk.rs`) follows a value to where it comes from. It starts from a picked variable, or from a risky call's arguments that matter (`memcpy`'s source and length, `sprintf`'s format and values), skipping constant ones.

### Reaching definitions, by syntax

There is no control-flow graph. `defs.rs` approximates **reaching definitions** from the syntax tree:

1. **Every write** to the name in the function, classified:

   | Kind | Examples |
   |---|---|
   | Set | a declaration with a value, `x = …`, a range-`for` variable |
   | Grows | `x += …` |
   | Part | `x[i] = …`, `*x = …`, `x->f = …` (index-insensitive) |
   | Declared | a declaration with no value |
   | Copied | a library call writing into it: `strcpy`, `memcpy`, `sprintf`, `sscanf`, `memset`, or a method such as `append` or `push_back` |
   | Read | an input source filling it |
   | Passed | handed to a project function that may write it |

2. **Filtered** to writes that can reach the use:
   - writes to the same variable, not a shadowing one in another block;
   - writes to a compatible field;
   - not writes in the opposite arm of an `if` or `?:` (unless a loop holds both).
3. **Ordered:** earlier writes latest first, plus writes later in a loop around the use (the loop brings them back).
4. **Killed:** walking back, the first write that **certainly replaces** the value ends the list.
   - "Certainly" means it isn't inside an `if` arm, a loop body, a `case`, a `catch`, a lambda, a `try` body, or the right side of `&&`/`||`.
   - It also means no `break`, `continue` or `goto` can skip it.
   - So writes that may run are all kept, and only one that always runs hides those before it.

### The search

A recursive **depth-first search**. Each step follows the reaching writes' values: into the variables they read, into calls, or to an end. The current path is a stack; a value already on the path isn't followed again (cycles end), and total work is bounded rather than tracked with a global visited set.

- **Parameters:** to the matching argument at each call site (up to 8 callers). If the walk stepped into this function from a call, it goes back to *that* call only. This is a **call-string** context, which keeps returns matched to their calls.
- **Calls:**
  - A call to an input source ends the path at "outside input".
  - A project function is stepped into, and each value it returns is followed.
  - A library call's result is assumed to come from its arguments that carry data (`strlen(s)` from `s`).
  - A method's result comes from its object.
- **Out-parameters:** a value written by a project function through a pointer is followed into that function, counting only writes through the pointer.
- **Ends:**
  - outside input (`argv`, `fgets`, `read`, `getenv`, `std::cin`);
  - constants: literals, `sizeof`, `NULL`, and all-caps names taken as macros;
  - a declaration with no value;
  - globals and members, which are written elsewhere;
  - function pointers and functions outside the project;
  - limits.

Paths are deduplicated and shown with those reaching input first, then shortest first.

**Bounds:**

| Limit | Value |
|---|---:|
| Time | 750 ms |
| Paths | 24 |
| Values followed | 2,000 |
| Steps in a path | 40 |
| Depth of the search | 64 |
| Calls stepped into | 8 deep |
| Callers per parameter / definitions per call / returns per function | 8 / 4 / 6 |
| Reaching writes per use | 6 |

Same-named functions are one symbol in the index, so the tracer guesses which one a call means: a definition in the caller's file first, and a `static` function outside a header only from its own file.

## 6. Tracing forward, and source → sink

**Trace Where It Goes** runs the same machinery forward (`src/analysis/trace/forward/`): from a variable, through each use that the value still reaches (by the same reaching-definitions check), to where the use takes it:
- stored in another variable or field;
- passed to a call:
  - into a project function's parameter;
  - a library copy's destination;
  - the result of a library call;
- returned to the call stepped in from, or to every caller;
- or used without being passed on (a condition, an index).

A **return stack** keeps returns matched to calls. A path is recorded each time the value reaches a **sink**, an argument of a risky call that matters, and following goes on past it.

**Source → sink** runs the forward tracer with only the call you pick as the sink, and shows the paths that reach it, or else where the value was lost.

Both are described step by step in [matching-and-tracing.md, Part 2](matching-and-tracing.md#part-2-source--sink-tracing).

## 7. Fuzz targets

`fuzz_targets` scores each function with **additive points**, each with a reason the Triage window shows.

**Worth** (is it worth fuzzing):

| Signal | Points |
|---|---:|
| Name has a parser word (parse, decode, unpack, inflate, lex, tokenize, …) | +3 |
| Otherwise an input word (read, recv, handle, packet, header, …) | +1 |
| Findings inside it: High / Medium / Low | +2 / +1 / 0, up to 6 |
| Cyclomatic complexity over 10 / 20 / 50 | +1 / +2 / +3 |
| Fed by outside input (input reachability), and takes a buffer or string | +2 |
| In an example, test, demo, sample or bench folder | −2 |

**Ease** (can a harness call it):

| Signal | Points |
|---|---:|
| Best input: a buffer and its length / a string or byte vector / a buffer alone | +4 / +3 / +2 |
| Each file-name parameter | −2 |
| No input parameter / no parameters at all | −2 / −3 |
| Each value a harness must build by hand | −2, up to −6 |
| A method (needs an object) / a non-member `static` / variadic | −2 / −1 / −1 |

Candidates need a total of 4, or worth of 6. `main`, operators, destructors and helpers (error, log, usage, …) are skipped.

**Harness drafts** map each parameter to a fill: bytes, a C string, `std::string`, a byte vector, a number, or by hand. A buffer is paired with the integer right after (or before) it if that is named or typed like a length. The draft is a `LLVMFuzzerTestOneInput` using `FuzzedDataProvider`:
- numbers first;
- then buffers, the last taking the remaining bytes;
- lengths from the buffers' sizes;
- a TODO wherever a value must be built by hand.

Names from the project go into the draft only if they can't break out of it (no quotes, `..`, absolute paths, trigraphs).

The **AFL++ draft** runs the same statements in a `static` function, called from a `main` in AFL++'s persistent mode: `__AFL_FUZZ_INIT`, then `__AFL_INIT` (the fork server starts there), then `__AFL_LOOP(10000)` around each input, read from `__AFL_FUZZ_TESTCASE_BUF` (memory shared with `afl-fuzz`). The `main` is chosen at compile time: `#ifdef __AFL_FUZZ_TESTCASE_LEN` (defined by AFL++'s compilers) picks that one; otherwise a `main` reads one input from stdin, so a crash replays with any compiler.

**Dictionaries** list the constants a fuzzer should splice into inputs:
1. The function and the functions it calls, breadth-first over call edges, two calls deep (at most 100), nearest first.
2. **Compared:** the constant arguments of calls such as `strcmp`, `strncmp`, `memcmp`, `strstr`, `strchr` and `strcspn` (library calls, and in-tree definitions of them). For calls taking a set of characters (`strspn`, `strcspn`, `strpbrk`, `strtok`, `strchr`, …), each character is a token, and the whole set isn't unless it is also compared whole. `sscanf` formats aren't tokens.
3. **Written:** each function's character constants (found by a small lexer that skips comments, strings, raw strings, `L'x'` and C++ digit separators like `1'000`), then its string literals that aren't printf formats.
4. Literals are decoded as the compiler would: escapes (`\n`, `\x41`, `\101`, `\u00e9`), adjacent literals joined, `u8` and raw strings; wide strings are skipped.
5. Repeats, empty tokens, lone NUL bytes and tokens over 128 bytes (AFL++ refuses those) are dropped, and the list stops at 200. Bytes outside printable ASCII are written as `\xNN`, which both AFL++ and libFuzzer read.

**Makefiles** list the files a fuzzer must link:
1. A breadth-first walk from the function over call and use edges. Each function maps to one defining file: of several (a version per architecture), the one sharing the most leading folders with the function's own file.
2. Linking works by file, so when a file is added, every function in it joins the walk too.
3. A name defined in more than two files is taken for a macro written like a function (`CMD_FUNC(cmd_join) { … }`) and not followed: as one symbol, it would carry every module's handler body. Versions under `#if` in one file count once.
4. Files under a `windows`, `win32`, `win64` or `msvc` folder are left out. Files with no function definitions in the folders already listed are added, for their globals (the index has no variables).
5. Non-static functions (other than `main` and macro-made names) defined in two listed files are reported as clashes.
6. `-I` folders: a breadth-first walk over the `#include` lines of the listed files, resolving each name as **G** does (beside the including file, else the project file ending with it that shares the most folders). A folder is needed when the header isn't beside the file including it: the resolved path minus the name as written.
7. At most 300 files and 2,000 scanned for includes. Paths go in only if they are made of letters, digits and `/._-+`, so nothing is read as make syntax.

Measured on UnrealIRCd (`b64_decode`, in `support.c`): the walk gives 49 of its 57 core files plus `dispatch.c` and `ircd_vars.c`, leaves out the Windows sources and `include/windows`, and flags `url_curl.c` against `url_unreal.c`. With one of those removed, `ircd.c` and the four files only it calls added, and the configure-generated `version.c`, it links and fuzzes at about 69,000 runs a second. (The harness also needed its `#include` swapped for the function's prototype: UnrealIRCd's headers aren't valid C++.)

## 8. Code matching

Described fully in [matching-and-tracing.md, Part 1](matching-and-tracing.md#part-1-semantic-code-matching). In short:
- **Shapes:** each function's tokens with every name made `$`, number `0`, string `""`, and comments dropped. Equal shapes are look-alikes. Renaming, new constants and reformatting don't change a shape; a new check or operator does.
- **Blocks** inside functions get their keys from a **polynomial rolling hash** over the function's tokens, so any block's key costs O(1) and deep `else if` chains don't go quadratic.
- **Code like one function:** every run of 5 tokens is a **shingle**. Each function keeps its 64 smallest shingle hashes, a **bottom-k MinHash sketch**. Two sketches estimate the **Jaccard index** of their shingle sets: exact for small functions, the standard bottom-k estimate otherwise. Only functions within 2.5× the size are compared, and under 40% is dropped.
- **Behavior tags:** each function is tagged by the library calls it makes (reads input, copies memory, runs programs, crypto, … 19 in all). Optionally the tags of the functions it calls are included, by breadth-first search up to 3 calls deep.

## 9. Semgrep rules from code

`src/analysis/similar/` drafts a semgrep rule from code you pick.

- **One line:** the call (or statement) on it. **Same shape** turns each variable into a metavariable (`$A`, `$B`, …; the same variable is always the same one) and keeps called names, fields, types, literals and capitalized macro names. **Any values** keeps only the call: each argument a metavariable, a string `"..."`, a nested call `<... f(...) ...>`. A draft of variables alone (`x = y;`) is refused, since it would match almost anything. This walk is iterative, so any nesting depth is safe.
- **Several lines:**
  1. The statements on the lines are picked: the one statement or function they hold exactly, else the statements starting on them in the innermost block holding them all.
  2. They become a sequence with `...` between them.
  3. One metavariable table is shared across them, so `p = malloc(n); … memcpy(p, s, n);` follows the same `p`.
- **Several places:** **anti-unification** (finding the most specific pattern that matches every place).
  - The places' syntax trees are walked side by side. Where all places have the same token, it stays.
  - Where they differ, the smallest expression holding the difference becomes a metavariable, one per distinct combination of texts, so a value used twice stays one metavariable. Differing strings become `"..."`, differing called names, fields, types or numbers a metavariable.
  - Argument lists of different lengths keep the arguments they share, then `...`.
  - Statements are lined up by a **longest common subsequence** on (kind, first call made), computed one place after another. Statements not in every place are dropped, with `...` where they were. Blocks of equal length that still differ are lined up the same way.
  - A failed sub-match rolls back the metavariables it handed out. The recursion is capped at depth 200, below which a differing subtree becomes one metavariable.
- **Exceptions** reuse the rule's metavariables (the draft records which variable each stands for):
  - a check `if (len > sizeof(buf))` becomes `pattern-not-inside: if (<... $C > sizeof($A) ...>) ...` followed by `...`;
  - the same call written safely becomes `pattern-not`;
  - any other statement becomes "not after this".

Every value taken from the code is written into the rule's YAML double-quoted with anything but printable ASCII escaped. A multi-line pattern of plain printable text is written as an indented block instead. Either way, a crafted line can't add keys or rules.

## 10. Wrappers of risky calls

`wrappers.rs` finds project functions that pass their own parameters straight to a risky call, so each call to the wrapper is in effect a call to the risky function:

1. For each external call and library call in tree, the caller's parameters are read from its header text (comments removed, brackets matched, `->` not taken for one). An argument matches a parameter when its text, a cast or parentheses aside, is the parameter's name.
2. What is passed decides the kind, and the rule that finds the calls worth a look:
   - **format** (printf and scanf families, the `v` forms included): calls whose format isn't a string literal;
   - **copy size** (`memcpy`, `strncpy`): calls whose size isn't `sizeof(...)`, a number or a CAPITALIZED constant, via `pattern-not` plus a `metavariable-regex` with a lookahead that excludes number literals;
   - **allocation size** (`malloc`, `kmalloc`): calls whose size is a product or a sum;
   - **anything else**, to a call the built-in rules flag whatever its arguments (`strcpy`, `system`): every call.
3. One wrapper per name: two same-named functions (static ones in different files) that pass on different things get the "every call" rule, which fits both.
4. Wrappers are ranked by how many places call them, counted from call occurrences. Only one level is followed: a wrapper of a wrapper isn't.

## 11. Git history

`src/history/` finds commits that say they fixed something, and the lines of today's code each one last changed.

- **Classifying commits:** a message's words, lowercased, matched whole against security words (overflow, use-after-free, oob, asan, fuzz, crash, leak, race, uninitialized, …, and `CVE-…` ids) and fix words (fix, bug, revert, workaround, …). The strongest match wins.
- **The walk:** from the checked-out commit, newest first by commit time, up to 20,000 commits. Each fix commit's files changed come from a tree diff against its first parent (merges list none). Up to 2,000 fix commits are kept.
- **Blame:** the files fix commits touched most are traced first (up to 2,000), each with gitoxide's blame. A file that differs from the commit is left out, since its lines can't be matched. Only lines owned by fix commits are kept. Files are blamed by a pool of up to 6 threads sharing a work counter. When the walk was cut short, blame stops at its oldest commit.
- **Isolation:** the repository is the audited project's, and gitoxide can't be interrupted. So the reading runs in a **separate process** (SrcWranglr itself with `--history-worker`), talking JSON over stdin and stdout, in its own process group, killed at a deadline (90 s + 30 s grace) or when you open another project. The repository is opened with no system or user config and its own config untrusted, and a `.git` with symlinked, oversized or borrowed parts is refused. The app re-bounds the answer it gets back.
- **CVEs:** ids in fix commits' messages, validated as `CVE-YYYY-NNNN` (4 to 7 digits), grouped with a hash map in the order first seen. The NVD link is rebuilt from the validated id.

## 12. SARIF import

`sarif.rs` reads SARIF 2.1 with typed structures, skipping fields it doesn't use unread.
- **Paths:** a result's URI, percent-decoded, `\` turned to `/`, a `file:` URI accepted only for this machine (no host). Joined to its base if it has one, it must be inside the project's folder (as written or canonical, with a `/` boundary) and must not climb with `..`. It must also be a file the index knows; everything else is counted as outside.
- **Columns:** SARIF counts UTF-16 units by default (or code points, if the run says so), 1-based. They are converted to byte columns using the line's text.
- **Severity:** CodeQL's `security-severity` if given (7 and up high, 4 and up medium), else the level (`error`, `warning`, `note`). The CWE comes from tags such as `external/cwe/cwe-120`.
- **Bounds:** 64 MB per report, 200,000 results, 50 on one line, 50 tools. A message is cut before its `[text](1)` links are read. A tool may not take the name of one of SrcWranglr's own (it would replace their findings); it gets "(imported)".

## 13. Project survey

- **Capabilities** (`analysis/capabilities.rs`): calls classified into 19 categories (sockets, shared memory, D-Bus, XPC, process creation, dynamic loading, …).
  - By exact name, with the Windows `A`/`W` suffixes removed, or by prefix (`dbus_`, `xpc_`, `NtAlpc`, `zmq_`).
  - Some by argument: `socket`'s address family tells Unix, netlink and network sockets apart; `mmap` counts as shared memory only with `MAP_SHARED`.
- **Binaries:** files sniffed by magic bytes (ELF, PE, Mach-O and fat Mach-O; Java class files told apart by their arch count), parsed with `goblin` inside a panic guard.
  - **ELF:** PIE, NX (`PT_GNU_STACK`), RELRO (partial or full with bind-now), stack canaries (`__stack_chk_fail`), FORTIFY (`__*_chk` symbols), RPATH, stripped, debug info.
  - **PE:** ASLR (and high-entropy on 64-bit), DEP, CFG, SEH, /GS, Authenticode present.
  - **Mach-O:** PIE, executable stack, canaries, FORTIFY, code signature, encryption, one report per slice.
  - Imports are classified like calls.
- **Complexity** per function is the cyclomatic complexity from indexing.

## 14. Cross references and highlighting

- **Cross references** (X over a name):
  - a project symbol's are every occurrence the index recorded, with its role (call, use, declaration) and the function holding it;
  - a library function's are every external call with that bare name.
  - Up to 200,000 are listed, sorted by file and place. The window builds its rows once, filters them only when the filter changes, and reads each visible row's line through a per-file index of line starts.
- **Highlight:** a click finds the identifier around the clicked character. Every whole-word occurrence on each drawn line is then marked, up to 100 per line.
