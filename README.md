# jq-for-windows

An independent, memory-safe implementation of the `jq` command-line JSON processor in Rust, designed to feel native on Windows x64 and ARM64.

The project aims for **95% practical compatibility** with jq 1.7: the commonly used language, command-line interface, output, errors, and exit behaviour. Compatibility is measured by black-box tests against an installed upstream `jq`; it is not based on copied source code.

## Why this project exists

`jq` is one of those rare tools that is both small at the command line and remarkably deep as a language. This project is an expression of respect for that design. Our goal is to make jq-compatible JSON processing feel first-class on Windows, while learning from the language and independently rebuilding its observable behaviour in modern, idiomatic Rust.

Compatibility here means respecting the contract that jq users rely on: filters produce streams, pipes preserve that model, function arguments are filters, updates operate through paths, and errors and exit behaviour matter. It does not mean copying jq's implementation or pretending that two very different languages should have identical internals.

The original jq remains the reference and the source of the ideas that make this project useful. We gratefully credit Stephen Dolan and the jq maintainers and contributors for creating and stewarding it. If this implementation becomes excellent, that excellence still begins with the language they designed.

## Project direction and implementation

This project is directed by **Harald**, who defines the product goals, compatibility target, priorities, design values, and release decisions.

The implementation has been carried out by **OpenAI GPT-5.6 Sol through Codex**, working under Harald's direction. This includes the Rust architecture, parser and evaluator work, CLI implementation, tests, documentation, and iterative debugging. The collaboration is intentionally AI-forward and is part of the project's identity, not something hidden behind a generic “AI-assisted” footnote.

The division of responsibility is therefore explicit:

- **Project direction and decisions:** Harald
- **Implementation and technical execution:** OpenAI GPT-5.6 Sol through Codex
- **Original language and compatibility reference:** jq, created by Stephen Dolan and maintained by the jq community

Model-generated work is still expected to meet ordinary open-source engineering standards: reviewable source, reproducible builds, structured tests, documented limitations, and evidence for compatibility claims. Model attribution does not replace maintainership or human responsibility for publishing and accepting changes.

## A Rust interpretation of jq

This is a from-scratch reimplementation, not a transliteration of the jq source. We deliberately use Rust-native and partly functional techniques where they fit the semantics:

- Filters are represented by an explicit, strongly typed AST rather than ad-hoc callbacks or copied parser structures.
- Evaluation is modelled as a transformation from one JSON value to a stream of zero or more results.
- Lexical variables use immutable, persistent scope chains backed by `Rc`; adding a binding shares its unchanged parent environment in O(1).
- User functions receive filters as arguments, preserving jq's higher-order behaviour instead of reducing parameters to eager JSON values.
- Updates share one path engine for assignment, modification, deletion, and the path built-ins.
- Errors are structured enums propagated through `Result`; invalid jq programs must not become panics or silently disappear.
- JSON transformations return new values. Local mutation is confined to construction and path-update internals, where ownership makes it explicit and safe.
- The crate forbids unsafe Rust. Portability should come from clear ownership and ordinary Rust abstractions, not platform-specific memory tricks.

These choices are not claims that our internals are better than jq's. They are how we make the implementation understandable, testable, and natural in Rust while remaining faithful to jq at the language boundary.

We also do not intend to preserve every historical accident merely for a headline percentage. Compatibility differences will be measured, documented, and decided consciously. The 95% target is a testable engineering goal, not a marketing shortcut.

## Status

This is an early but working implementation. It currently supports:

- JSON streams on standard input
- identity, chained object fields, integer indices, iteration, and slicing
- JSON literals, array constructors, and object constructors
- pipes (`|`), multiple-result filters (`,`), parentheses, and optional access (`?`)
- arithmetic, comparison, boolean, and alternative (`//`) operators
- checked runtime errors for invalid types and division by zero
- core built-ins: `length`, `type`, `keys`, `has`, `map`, `select`, `empty`, `error`, and `tostring`
- lexical variables and stream binding with `$name` and `as $name | ...`
- user-defined `def` functions with filter parameters, lexical definition scope, and guarded recursion
- control flow with `if`/`elif`/`else`, `try`/`catch`, `reduce`, and `foreach`
- path updates with `=`, `|=`, compound assignments, `del`, `getpath`, `setpath`, and `paths`
- collection and text built-ins including sorting, grouping, uniqueness, aggregates, flattening, containment, splitting, and joining
- string interpolation, regular-expression matching and replacement, JSON parsing, and `@json`/`@csv`/`@tsv`/`@uri`/`@base64` format filters
- `--compact-output`, `--raw-output`, and `--null-input`
- file input, `--raw-input`, `--slurp`, `--exit-status`, `--arg`, and `--argjson`

## Differential compatibility testing

The integration corpus in `tests/differential.rs` evaluates the same filters and JSON values with this crate and an upstream jq executable, then compares the resulting JSON streams structurally. Set `JQ_REFERENCE` to the path of jq when it is not available on `PATH`:

```powershell
$env:JQ_REFERENCE = 'C:\tools\jq.exe'
cargo test --test differential -- --nocapture
```

For the pinned Windows x64 reference used during development:

```powershell
.\tools\fetch-reference-jq.ps1
$env:JQ_REFERENCE = (Resolve-Path .\tools\reference\jq-1.7.1-windows-amd64.exe).Path
cargo test --test differential -- --nocapture
```

The helper downloads the official jq 1.7.1 release from the jq GitHub repository and verifies SHA-256 `7451FBBF37FEFFB9BF262BD97C54F0DA558C63F0748E64152DD87B0A07B6D6AB`. The executable is ignored by Git.

The test reports a skip when no reference executable is available. CI and compatibility reports should provide a pinned jq version so a missing reference cannot be mistaken for measured compatibility.

## Build

```powershell
cargo build --release
'{"user":{"name":"Ada"}}' | target\release\jq.exe -r '.user | .name'
```

Rust's `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc` targets are the primary release targets.

## Compatibility roadmap

1. Core parser and value-stream evaluator
2. Literals, constructors, iteration, slicing, and optional access
3. Operators, structured runtime errors, variables, functions, control flow, and path updates
4. Broader built-in coverage and jq-compatible CLI/input modes (in progress)
5. Differential conformance suite and performance hardening

The 95% target will be reported against a versioned public corpus. Unsupported behaviour must be documented rather than silently approximated.

## Relationship to jq

This project is inspired by and seeks command-line compatibility with [jq](https://jqlang.org/), created by Stephen Dolan and maintained by the jq community. jq is a separate project and is distributed under the MIT license. This repository is an independent implementation written from scratch; it is not endorsed by or affiliated with the jq project.

The name `jq` is used to describe the compatibility target and expected command-line interface. Project documentation and releases should always make the independent relationship clear.

## License

Licensed under the MIT license.
