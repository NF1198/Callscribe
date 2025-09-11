# Callscribe

## Functional Overview

**Callscribe** is a high-performance, asynchronous Rust application for parsing [DSDPlus](http://www.dsdplus.com/) SRT log files and converting them into structured CSV output.  

The tool is designed to:
- Stream parse `.srt` and `.event`.
- Apply flexible filters (frequencies, radio types, radio IDs, talkgroups, NACs).
- Enrich parsed records with transcriptions from associated recording artifacts (e.g., pre-transcribed `.txt` files).
- Output Excel-compatible CSV files suitable for further analysis and archival.

It is built with a modular, streaming pipeline:
```
(SRT/EVENT) Parser → Filter → Transcription Adder → CSV Sink
```

Each stage is asynchronous and communicates via bounded channels, enabling parallelism and backpressure handling.

---

## Disclaimer

This software is an **independent project**.  
It is **not affiliated with, endorsed by, or related to DSDPlus, DSD, or the authors/maintainers of those projects**.  

All references to DSDPlus are descriptive only, to clarify the format of log files that this tool is capable of parsing.

---

## Usage

### Command line

```bash
dsd_event_parser [OPTIONS] <INPUT_FILES>...
```

### Options

| Flag / Option | Description |
|---------------|-------------|
| `-f, --freq <FREQ>` | Filter for one or more frequencies (exact match on MHz, e.g. `153.450000`). |
| `-t, --rtype <TYPE>` | Filter for one or more radio types (e.g. `DMR`, `P25p1`, `P25p2`). |
| `-r, --rid <RID>` | Filter for one or more radio IDs. |
| `-g, --tg <TG>` | Filter for one or more talk groups. |
| `-n, --nac <NAC>` | Filter for one or more NACs. |
| `--tz <IANA_TZ>` | Override local timezone with a specific IANA timezone string (e.g., `America/New_York`). |
| `--log <LEVEL>` | Logging verbosity: `essential` (default), `debug`, `trace`, `warn`, `error`. |
| `--record-dir <DIR>` | Path to a “Record” directory containing dated subfolders (e.g. `20250910/064356_153.450000_...txt`). |
| `--transcriber <ENGINE>` | Transcription engine to use. Currently only `text` is supported (reads `.txt` transcripts). |
| `<INPUT_FILES>` | One or more `.srt` / `.event` files to parse. |

### Example

```bash
dsd_event_parser \
  --freq 153.450000 \
  --rtype P25p1 \
  --rid 4506 \
  --record-dir ./Record \
  --transcriber text \
  --log debug \
  CC-DSDPlus.event
```

---

## Note on Transcription

The **TextFileTranscriber** is the default transcription engine.  

It does not perform speech-to-text itself, but instead looks for `.txt` files with the same base naming convention as the audio files (e.g. `064356_153.450000_004_P25__GC_2_4506.txt`).  

This means you can preprocess your recordings with an external transcription tool, such as [OpenAI Whisper](https://github.com/openai/whisper) or [faster-whisper](https://github.com/guillaumekln/faster-whisper), and save the results alongside your audio files as text files. When present, **DSD Event Parser** will automatically integrate those transcripts into the CSV output, populating the `Slot1.Text` or `Slot2.Text` fields.

---

## Code Philosophy

The parser is written in **idiomatic, async Rust** with an emphasis on:
- **Streaming**: avoids whole-file loads (`BufReader` line streaming).
- **Pipeline architecture**: clear separation of responsibilities (`srt_stream`, `filter`, `transcription_adder`, `csv_sink`).
- **Backpressure**: bounded channels prevent runaway memory growth.
- **Pluggability**: transcription is abstracted by the `Transcriber` trait.
- **Excel-compatibility**: fields like `+DMR` or `+P25p2` are guarded against being misinterpreted as formulas.

This design makes the tool scalable, debuggable, and extensible.

---

## Extension Points

1. **Transcribers**  
   - Trait: `Transcriber` (`transcriber.rs`)  
   - Current: `TextFileTranscriber` (reads `.txt` transcripts, lazily indexed).  
   - Future: could add Whisper (OpenVINO), cloud STT APIs, or hybrid approaches.

2. **Filters**  
   - Implemented in `filter.rs`.  
   - Easy to extend with new criteria (e.g., encryption type, slot metadata).  

3. **SRT Parsing**  
   - Stream parser in `srt_stream.rs`.  
   - Currently assumes consistent SRT structure (record #, time range, datetime, freq/type lines).  
   - Can be extended to support other formats or variants.

4. **CSV Output**  
   - Implemented in `csv_sink.rs`.  
   - Currently Excel-friendly CSV.  
   - Could be swapped for JSON, SQLite, or parquet sinks with minimal changes.

5. **Indexing Strategy**  
   - The incremental indexer in `TextFileTranscriber` only loads transcript filenames for the needed day (`YYYYMMDD`) on first use.  
   - This keeps startup fast while avoiding per-record filesystem scans.

---

## Development Notes

- Requires **Rust 1.70+** and `cargo`.
- Dependencies include `tokio`, `csv-async`, `chrono`, `chrono-tz`, `walkdir`, and `thiserror`.
- Logging controlled via `env_logger`.

### Build & Run

```bash
cargo build --release
cargo run -- --help
```

### Install to PATH

```bash
cargo install --path .
```

This places `dsd_event_parser` in your cargo bin (usually `~/.cargo/bin`).

---

## License

```
MIT License

Copyright (c) 2026 Nicholas Folse

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
