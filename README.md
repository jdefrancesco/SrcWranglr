# SrcWranglr

**Explore C/C++ codebases through interactive graphs, linked source, and code auditing tools.**

SrcWranglr is a free, MIT-licensed desktop application for getting oriented in unfamiliar C and C++ projects. Open a folder, pick a symbol, and follow its callers, dependencies, and uses alongside the source. Keep notes as you investigate, trace values across functions, and identify code worth reviewing or fuzzing.

Written in Rust with [egui](https://github.com/emilk/egui) and [tree-sitter](https://tree-sitter.github.io/). Inspired by [Sourcetrail](https://github.com/CoatiSoftware/Sourcetrail), with an independent implementation that shares no code with it.

![SrcWranglr showing interactive code graphs, linked source, findings, project triage, and fuzz targets](docs/screenshots/slideshow.webp)

[Quick start](#quick-start) · [Features](#features) · [Features planned](#features-planned) · [Limitations](#limitations) · [User guide](docs/guide.md) · [AI policy](#ai-policy)


## About

I built **SrcWranglr** to help me understand C/C++ codebases while learning Rust. The core analysis code was written by hand; AI tools have assisted with GUI work, tests, and documentation.

Before the Rust re-write, the project used the working name *slurcetrail* and was written in Python for a prototype, then C, then I got side tracked by other things.

## Quick start

**Requires Rust 1.95 or newer.** Development and testing currently happen on macOS. Linux and Windows are not yet tested.

```bash
git clone https://github.com/jdefrancesco/SrcWranglr.git
cd SrcWranglr
cargo run --release --locked
```

Try the built-in demo from the start page, or choose **File → Open Project…** and select a C/C++ project folder. You can also open a project directly:

```bash
cargo run --release --locked -- ~/code/myproject
```

Indexing reads source directly; you do not need to build the project or provide a compilation database. It runs in the background, supports cancellation, and respects `.gitignore`. See [limitations](#limitations) for indexing and analysis boundaries.

`Semgrep`, `cppcheck`, and `flawfinder` are **optional**. Install them separately to use their integrations; the graph, navigation, and built-in analysis work without them. **File → Settings…** shows tool discovery and editor options.

## Features

### See how the code fits together

The **Trail** view keeps the selected symbol in the center, its callers on the left, and what it calls or uses on the right. Members sit inside their class boxes, and large groups show a hidden-item count to keep the view readable.

- Follow calls, inheritance, type and member uses, and containment.
- Switch to **Call hierarchy** to explore a tree of calls from entry points or a function you select. Callees appear in source order, with recursion and additional callers marked.
- Click a resolved name in the source or a node in the graph to explore it. Step through declaration and use locations, then go back through your navigation history.
- Press **X** over any name for IDA-style cross references: every use and call, with the function each is in. It works for library functions like `memcpy` too.
- Click any name in the source to highlight it everywhere in the file.
- Jump to a symbol with fuzzy search, pan and zoom the graph, and open the current location in your preferred editor.

### Investigate code and find audit leads

| Task | What SrcWranglr provides |
|---|---|
| Review risky calls | Argument-aware checks of about 300 C library, POSIX, Windows, Linux kernel, and weak-crypto functions, a custom dangerous-functions list, and findings from `Semgrep`, `cppcheck`, and `flawfinder`. Filter results and record verdicts. |
| Import CodeQL results | Load SARIF reports from CodeQL or any other tool into Findings, with severity, CWE, and verdicts. SrcWranglr never builds your code or runs CodeQL itself. |
| Follow a value | Trace backward to its origin, forward to risky calls, or between a source variable and a call you choose. Each path links to its source lines. |
| Find related code | Draft `Semgrep` rules from a line, a range of lines, or several places that share a pattern, with exceptions for checks and fixed code. Manage your own and the project's rules in the **Semgrep Rules** window, which also suggests rules for the project's wrappers of risky calls. Find functions and blocks with similar syntax, or filter functions by library-call behavior. |
| Choose fuzz targets | Rank candidates using input reach-ability, risky calls, complexity, and signatures. Draft editable `libFuzzer` harnesses, with `TODO`s where manual work is needed. |
| Survey a project | Inspect languages, build metadata, function complexity, IPC and capabilities, and ELF/PE/Mach-O binaries with hardening information. |
| Review past fixes | Find commits whose messages suggest bug or security fixes, inspect the lines they still own in the current checkout, and list the CVEs they name, with links to NVD. |

Input reach-ability helps prioritize findings. Value traces follow assignments, calls, returns, copies, out-parameters, and struct fields where supported. Both use syntax and heuristics; review the source to establish what a path means.

See the [user guide](docs/guide.md) for analysis behavior, examples, tool configuration, and limits.

### Keep your investigation organized

- **Notes, bookmarks, and TODOs:** attach context to symbols and source lines, and browse TODO-style comments already in the code.
- **Sessions:** reopen a project where you left off, or save a `.srcwranglr-session` file.
- **Audit bundles:** export notes, bookmarks, TODOs, verdicts, and your exploration position to a `.srcwranglr-audit` file. A teammate can preview and merge it into their own checkout.
- **Strings and documentation:** find string literals and their use sites, read project doc comments and Doxygen documentation, look up library functions in system man pages, or click the project's name to read its README, rendered from Markdown.
- **Customizable workspace:** arrange panes, choose source themes, symbol-list and top-bar fonts, and configure your editor and tool locations.

## Essential controls

| Action | Control |
|---|---|
| Open a project | ⌘O / Ctrl+O |
| Find a symbol | ⌘P / Ctrl+P |
| Open the command palette | ⌘⇧P / Ctrl+Shift+P |
| Explore a symbol | Click it in the graph, symbol list, or linked source |
| Cross references to a name | Point at it in the source and press **X** |
| Highlight a name in the file | Click it in the source; **Esc** clears |
| Return to the previous symbol | **◀ Back** |
| Trace a value, find similar code, or show docs | Right-click a source line (Shift-click line numbers to pick several) |
| Manage semgrep rules | **Analyze → Semgrep Rules…** |
| Open Project Triage | ⌘⇧T / Ctrl+Shift+T |
| Open settings | ⌘, / Ctrl+, |

See [all controls](docs/guide.md#controls) for pane layout, sessions, and analysis actions.

## Limitations

SrcWranglr uses tree-sitter for syntax-based indexing and heuristic name resolution. These boundaries matter when interpreting the graph and analysis:

- **Symbol identity:** overloads with the same name merge into one symbol. Same-named `static` functions in different files also merge in the index, which can affect graph relationships and reachability.
- **Language coverage:** macros are not expanded and `using namespace` is not followed. Global variables, enums, and general `typedef`/`using` aliases are not indexed yet; named `typedef struct` declarations are supported. Ambiguous references are left unlinked.
- **Analysis coverage:** unresolved indirect calls, function pointers, pointer aliases, and checks that constrain a value are not fully modeled. Reachability can include calls that do not receive the input, and a finding without an input tag is not proven unreachable. A displayed trace describes a possible path, not a confirmed vulnerability.
- **Resource limits:** indexing defaults to 100,000 source files and skips files larger than 2 MiB with a warning. Analysis also has time and traversal limits and reports when a trace is cut short.
- **Harness drafts:** generated fuzz harnesses need review and may need changes before they compile, particularly around macros, include order, and project-specific types.

Detailed behavior is documented in the [user guide](docs/guide.md#limitations).

## Build a package

```sh
make package
```

This builds and packages SrcWranglr for the host platform, placing the results in `dist/`.

| Platform | Package | Installation |
|---|---|---|
| macOS | `.app` bundle and `.dmg`, ad-hoc signed by default | `make install` copies the app to `/Applications` |
| Linux | `.tar.gz` with the binary, desktop entry, and icons | `make install` uses `/usr/local` by default; supports `PREFIX` and `DESTDIR` |
| Windows | `.zip`; run `make` from Git Bash or MSYS2 | Extract the archive |

Packaging support for Linux and Windows is present; those platforms remain untested. See [packaging details](docs/guide.md#building-a-package) for signing and other build targets.

## Command line

```text
srcwranglr [PROJECT_DIR] [--symbol QUALIFIED_NAME] [--view trail|calls] [--triage]
srcwranglr --version
```

For example, open **LevelDB** at a function in the call hierarchy:

```bash
srcwranglr ~/code/leveldb --symbol leveldb::DBImpl::Write --view calls
```

Use `--triage` to open the project survey after indexing. See the [command-line reference](docs/guide.md#command-line) for all options.

## Development

Run the format check, Clippy, and tests:

```sh
make check
```

Inspect the syntax tree of a C/C++ file:

```sh
cargo run --example dump_tree -- path/to/file.cpp
```

The core pipeline is:

```text
discover files → extract syntax → resolve names → CodeGraph → layout → render
```

Indexing lives in `src/index/`, graph layout and rendering in `src/graph/`, value tracing and analysis in `src/analysis/`, and application behavior in `src/app/`. See the [source map](docs/guide.md#how-it-works) and [development reference](docs/guide.md#development) for individual modules, headless indexing measurements, and harness export commands.

Bug reports are especially useful with a small C/C++ example, the expected relationship or trace, the observed result, and your platform. [Open an issue](https://github.com/jdefrancesco/SrcWranglr/issues).

## Features planned

Planned additions include:

- Following values through pointer aliases.
- Richer semantic indexing and search beyond syntax shapes and call behaviors.
- Collecting useful seed corpora for fuzz targets.
- Exploring lightweight language-model assistance for understanding large codebases: explaining a function, triaging a finding, and drafting a rule from a description.


## AI Policy

This project was ported from software I had been working on years ago. I decided to perform a rewrite in Rust for learning if I am capable of writing real tools in Rust. Oh, also because "Rustaceans" are relentless zealots who pushed it so hard that
knowing it has become a requirement for a lot of system development. The cult of Rust can give it a rest now though that us unlikely to happen heheh...

The majority of the code is **manually written** with the exception of: 

1. The GUI. Writing UIs isn't something I find thrilling.
2. Documentation found in the repository.
3. The `Makefile` and various helper scripts.
4. Yak-shaving tasks that consume too much time for a one man army.

That being said, blind usage of LLMs aren't acceptable. AI has become very useful but I am a strong believer that humans
need to be driving and crafting a deep understanding of their code. Prototyping, helping debug difficult problems, and security review are
also acceptable uses of LLMs. So please, if you plan to contribute use AI as a tool and exercise discretion. AI Slop can easily derail otherwise awesome 
project, 

## Acknowledgements

- Inspired by [Sourcetrail](https://github.com/CoatiSoftware/Sourcetrail) by Coati Software, now archived. `SrcWranglr` is an independent implementation and shares no code with it.
- Screenshots show [LevelDB](https://github.com/google/leveldb) (BSD-3-Clause, © The LevelDB Authors).
- Bundles the fonts [Source Code Pro](https://github.com/adobe-fonts/source-code-pro) (© Adobe) and [Iosevka](https://github.com/be5invis/Iosevka) (© Renzhi Li), both under the SIL Open Font License 1.1, and [Ubuntu](https://github.com/google/fonts/tree/main/ufl/ubuntu) Regular and Medium (© Canonical Ltd.) under the Ubuntu Font License 1.0; see [assets/fonts](assets/fonts).
- Built on [egui/eframe](https://github.com/emilk/egui), [tree-sitter](https://github.com/tree-sitter/tree-sitter) with [tree-sitter-cpp](https://github.com/tree-sitter/tree-sitter-cpp), [ignore](https://github.com/BurntSushi/ripgrep/tree/master/crates/ignore), [rfd](https://github.com/PolyMeilleur/rfd), [nucleo-matcher](https://github.com/helix-editor/nucleo), [gitoxide](https://github.com/GitoxideLabs/gitoxide), and [pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark).

## License

[MIT](LICENSE) © 2026 Joseph DeFrancesco
