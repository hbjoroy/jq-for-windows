# jq 1.7.1 compatibility matrix

This document tracks the observable jq surface, not similarity to jq's implementation. The reference is the [official jq 1.7 manual](https://jqlang.org/manual/v1.7/) and the `builtins` list reported by the pinned jq 1.7.1 executable.

Status meanings:

- **Supported** — implemented and covered by a regression or differential corpus case.
- **Partial** — useful behaviour exists, but a documented jq form or semantic edge is missing.
- **Missing** — not accepted or not implemented.
- **Unmeasured** — code exists, but the public differential corpus is not broad enough for a compatibility claim.

The status counts are an engineering inventory, not a compatibility percentage. The 95% project target is measured against the versioned public corpus.

## Language and syntax

| Area | Status | Current coverage or limitation |
|---|---|---|
| JSON literals | Supported | null, booleans, strings, numbers, arrays and fixed-key objects |
| Identity, pipe and comma | Supported | value-stream and Cartesian semantics are tested |
| Field access | Partial | identifier fields and chains work; quoted and computed object indices are missing |
| Array indexing and slicing | Partial | constant integer indices and constant slice bounds work; filter-valued indices/bounds are missing |
| Iteration `.[]` | Supported | arrays and object values, including optional form |
| Optional operator `?` | Supported | suppresses evaluation errors |
| Array construction | Supported | collects a result stream |
| Object construction | Partial | fixed identifier/string keys work; shorthand, variable and computed keys are incomplete |
| String interpolation | Supported | interpolation preserves filter streams and JSON escaping |
| Arithmetic | Partial | numeric `+ - * / %` and common jq polymorphism; edge coverage is incomplete |
| Comparisons and booleans | Supported | total value ordering, equality, `and`, `or`, and `not` |
| Alternative `//` | Supported | stream-aware false/null fallback |
| `if` / `elif` / `else` | Supported | branch streams are preserved |
| `try` / `catch` | Supported | explicit and runtime errors; shorthand `try EXP` |
| Variables and `as` | Partial | lexical bindings and shadowing work; destructuring patterns are missing |
| User functions `def` | Partial | filter arguments, lexical scope and guarded recursion; no tail-call optimisation or modules |
| `reduce` | Supported | scalar variable binding form |
| `foreach` | Partial | update and extract forms work; destructuring forms are missing |
| Assignment and updates | Partial | `=`, `|=`, `+=`, `-=`, `*=`, `/=`, `%=`; `//=` and advanced multi-path edges remain |
| Recursive descent `..` | Missing | — |
| `label` / `break` | Missing | — |
| `while`, `until`, `repeat` | Missing | jq standard-library recursion helpers are not installed |
| Comments | Missing | jq source comments are not parsed |
| Modules and imports | Missing | `module`, `import`, `include`, search paths and module metadata |

## Built-in filters

### Supported

These signatures are implemented and represented in the public tests:

| Family | Signatures |
|---|---|
| Core | `length/0`, `type/0`, `empty/0`, `tostring/0`, `tonumber/0`, `fromjson/0`, `error/0`, `error/1` |
| Selection | `has/1`, `map/1`, `select/1` |
| Ordering and grouping | `sort/0`, `sort_by/1`, `group_by/1`, `unique/0`, `unique_by/1`, `min/0`, `max/0` |
| Aggregation | `add/0`, `flatten/0`, `flatten/1` |
| Containment | `contains/1`, `inside/1` |
| Strings | `startswith/1`, `endswith/1`, `split/1`, `join/1` |
| Paths and updates | `del/1`, `getpath/1`, `setpath/2`, `paths/0` |
| Regular expressions | `test/1`, `test/2`, `match/1`, `match/2`, `capture/1`, `capture/2`, `scan/1`, `scan/2`, `sub/2`, `gsub/2` |

`not/0` is supported as language syntax rather than represented as an AST builtin.

### Partial

| Family | Available | Known gap |
|---|---|---|
| Regular expressions | test, match, capture, scan, sub and gsub | Rust `regex` is not Oniguruma; replacement filters, all jq flags and `sub/3`, `gsub/3`, `splits` are incomplete |
| Paths | `paths/0`, `getpath/1`, `setpath/2`, `del/1` | `path/1`, `paths/1`, `delpaths/1` and filter-valued/computed path components are missing |
| JSON conversion | `tostring`, `tonumber`, `fromjson`, `@json` | named `tojson/0` is missing |
| Ordering | sort, group, unique, min and max | `min_by/1` and `max_by/1` are missing |

### Missing standard-library families

Every name below is present in jq 1.7.1's own `builtins` output but is not currently implemented.

| Family | Missing names/signatures |
|---|---|
| Predicates and type selectors | `any/0,1,2`, `all/0,1,2`, `isempty/1`, `arrays`, `objects`, `iterables`, `booleans`, `numbers`, `normals`, `finites`, `strings`, `nulls`, `values`, `scalars`, `isfinite`, `isnormal`, `isnan`, `isinfinite` |
| Sequence generation | `range/1,2,3`, `limit/2`, `first/0,1`, `last/0,1`, `nth/1,2`, `combinations/0,1`, `repeat/1` |
| Collection transforms | `map_values/1`, `to_entries`, `from_entries`, `with_entries/1`, `reverse`, `transpose`, `walk/1`, `bsearch/1`, `pick/1` |
| Searching | `indices/1`, `index/1`, `rindex/1`, `in/1`, `IN/1,2` |
| String/codepoint | `ltrimstr/1`, `rtrimstr/1`, `explode`, `implode`, `ascii_downcase`, `ascii_upcase`, `utf8bytelength` |
| Recursion | `recurse/0,1,2`, `while/2`, `until/2` |
| Date and time | `now`, `gmtime`, `localtime`, `mktime`, `strftime/1`, `strflocaltime/1`, `strptime/1`, `fromdate`, `todate`, `fromdateiso8601`, `todateiso8601` |
| Environment and process | `env`, `halt`, `halt_error/0,1`, `debug/0,1`, `stderr` |
| Input and origin | `input`, `inputs`, `input_filename`, `input_line_number`, `get_jq_origin`, `get_prog_origin`, `get_search_list` |
| Streaming | `tostream`, `fromstream/1`, `truncate_stream/1` |
| Modules | `modulemeta/0` and module-provided behaviour |
| Introspection | `builtins/0`, `$__loc__` |
| SQL-style joins | `INDEX/1,2`, `JOIN/2,3,4` |
| Numeric constants | `nan`, `infinite` |
| Mathematical functions | `abs`, `fabs`, `floor`, `ceil`, `round`, `trunc`, `sqrt`, `cbrt`, `exp`, `exp2`, `exp10`, `expm1`, `log`, `log2`, `log10`, `log1p`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `sinh`, `cosh`, `tanh`, `asinh`, `acosh`, `atanh`, `pow`, `pow10`, `hypot`, `fma`, `fmod`, `remainder`, `drem`, `fmin`, `fmax`, `fdim`, `copysign`, `frexp`, `ldexp`, `modf`, `scalb`, `scalbln`, `logb`, `significand`, `nextafter`, `nexttoward`, `nearbyint`, `rint`, `erf`, `erfc`, `gamma`, `tgamma`, `lgamma`, `lgamma_r`, `j0`, `j1`, `jn`, `y0`, `y1`, `yn` |

## Format filters

| Filter | Status | Note |
|---|---|---|
| `@json` | Supported | differential coverage |
| `@csv` | Supported | regression coverage |
| `@tsv` | Supported | regression coverage |
| `@uri` | Supported | regression coverage |
| `@base64` | Supported | differential coverage |
| `@text` | Missing | — |
| `@html` | Missing | — |
| `@sh` | Missing | — |
| `@base64d` | Missing | — |

## Command-line and I/O

| Option or behaviour | Status | Note |
|---|---|---|
| JSON input streams and files | Supported | multiple JSON values and file operands |
| `-n`, `--null-input` | Supported | differential CLI coverage |
| `-R`, `--raw-input` | Supported | platform-specific CRLF behaviour tested |
| `-s`, `--slurp` | Supported | JSON and raw slurp |
| `-c`, `--compact-output` | Supported | differential CLI coverage |
| `-r`, `--raw-output` | Supported | differential CLI coverage |
| `-e`, `--exit-status` | Supported | false/null/empty and error statuses tested |
| `--arg`, `--argjson` | Partial | variables work; `$ARGS.named` is missing |
| `-h`, `--help`; `--version` | Partial | accepted, but text/version branding intentionally differs |
| `--raw-output0`, `-j`, `-a`, `-S` | Missing | output modes |
| `-C`, `-M`, `JQ_COLORS`, `NO_COLOR` | Missing | colour handling |
| `--tab`, `--indent`, `--unbuffered` | Missing | output formatting/buffering |
| `--stream`, `--stream-errors`, `--seq` | Missing | streaming input modes |
| `-f`, `-L` | Missing | program files and module search path |
| `--slurpfile`, `--rawfile` | Missing | file-backed variables |
| `--args`, `--jsonargs`, `--` | Missing | positional argument modes |
| `-b`, `-V`, `--build-configuration`, `--run-tests` | Missing | platform/introspection/test modes |

## Evidence and maintenance

- `corpus/cases.json` is generated by `cargo run --example generate-corpus`.
- `tests/differential.rs` compares value streams and error phases with pinned jq 1.7.1.
- `tests/cli.rs` compares stdout, error presence and exit status with pinned jq 1.7.1.
- CI builds Windows, Linux and macOS on x64 and ARM64 and runs the pinned differential suite on Windows x64.

When a feature changes status, add or expand a differential case in the same change. “Supported” should mean observable evidence, not merely that a parser branch or enum variant exists.
