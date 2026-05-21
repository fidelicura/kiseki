Kiseki (軌跡, _"trajectory"_) is a Rust crate for programmatically producing [Kanata]-format processor pipeline traces, compatible with the [Konata] visualizer.

The Kanata format is a tab-separated plain-text log that records the pipeline events (fetch, rename, dispatch, execute, retire, etc.) of a processor simulation. It is intentionally independent of any specific ISA or microarchitecture, which lets the same viewer be reused across a wide range of simulators.

Kiseki provides a small, strongly typed API for emitting such a log without having to construct the text representation by hand, and optionally validates the produced stream against the lifecycle rules of the format.

> **NOTE**: Kiseki targets Kanata format version `0004`. Earlier revisions are not supported.

## Installation

```toml
[dependencies]
kiseki = "0.1"
```

## Quickstart

```rust
use kiseki::{Config, Level, Trace};

struct MyConfig;

impl Config for MyConfig {
    type Output = Vec<u8>;
    type Stage = &'static str;
}

fn main() -> kiseki::Result {
    let mut trace = Trace::<MyConfig>::new(0, Vec::new())?;

    let instruction = trace.start(0x1000, 0)?;
    trace.label(instruction, "add r1, r2, r3", Level::Pane)?;

    trace.stage(instruction, 0, &"F", false)?;
    trace.advance(1)?;
    trace.stage(instruction, 0, &"D", false)?;
    trace.advance(1)?;
    trace.stage(instruction, 0, &"X", true)?;
    trace.advance(1)?;
    trace.retire(instruction)?;

    let bytes = trace.finish()?;
    let output = std::str::from_utf8(&bytes).unwrap();
    print!("{output}");

    Ok(())
}
```

Resulting log:

```text
Kanata	0004
C=	0
I	0	4096	0
L	0	0	add r1, r2, r3
S	0	0	F
C	1
S	0	0	D
C	1
S	0	0	X_X
C	1
R	0	0	0
```

## Configuration

The [`Config`] trait selects three compile-time parameters:

- [`Config::Output`]: the [`std::io::Write`] sink the trace is written into. Use `Vec<u8>` for tests, a `BufWriter<File>` for on-disk traces, or any other writer of your choice.
- [`Config::Stage`]: the [`std::fmt::Display`] type used to name pipeline stages. A `&'static str` is the simplest choice; a custom `enum` implementing `Display` is recommended for larger simulators since it prevents typos in stage names.
- [`Config::VALIDATE`]: a `const bool` (default `true`) that toggles instruction-lifecycle validation. When enabled, the trace remembers every instruction it issued and reports lifecycle violations (referring to a retired id, retiring the same instruction twice, finishing the trace with outstanding instructions) as [`Error`] values. Disable it to compile the bookkeeping away when the simulator is already known to be well-formed.
- [`Config::SANITIZE`]: a `const bool` (default `true`) that toggles label sanitization. When enabled, every text passed to [`Trace::label`] is scanned for characters that would corrupt the line-based Kanata stream (`\t`, `\n`, `\r`) and rejected with [`Error::InvalidLabel`] before any output is written. Disable it to compile the scan away when label text is already known to be well-formed; unsanitized forbidden characters pass through verbatim and will break the log.

## Interface

The full Kanata command set is exposed as one method per command:

| Method              | Kanata command | Purpose                                  |
|---------------------|----------------|------------------------------------------|
| [`Trace::new`]      | `Kanata`, `C=` | Write the header and anchor cycle        |
| [`Trace::advance`]  | `C`            | Advance the simulation clock             |
| [`Trace::start`]    | `I`            | Introduce a new instruction              |
| [`Trace::label`]    | `L`            | Attach text to an instruction            |
| [`Trace::stage`]    | `S`            | Start a pipeline stage on a given lane   |
| [`Trace::end`]      | `E`            | Explicitly end a pipeline stage          |
| [`Trace::wake`]     | `W`            | Record a producer to consumer dependency |
| [`Trace::retire`]   | `R` (type `0`) | Commit an instruction                    |
| [`Trace::flush`]    | `R` (type `1`) | Squash an instruction                    |
| [`Trace::finish`]   |                | Validate and return the underlying sink  |

See the per-method documentation for details and validation behaviour.

## Contributing

Contributions are welcome. Please open an issue to discuss substantial changes before sending a pull request, and ensure `cargo test` and `cargo clippy` pass locally.

Common tasks are wrapped in the [`Justfile`](./Justfile) - install [`just`](https://github.com/casey/just) and run `just` to list recipes (`just test`, `just lint`, `just bench`, `just coverage`, `just doc`).

## License

Licensed under the Apache License, Version 2.0. See [LICENSE] for the full text.

<!-- Appendix -->

[Kanata]: https://github.com/shioyadan/Konata/blob/master/docs/kanata-log-format.md
[Konata]: https://github.com/shioyadan/Konata
[LICENSE]: ./LICENSE
