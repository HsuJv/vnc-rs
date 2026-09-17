# Protocol fuzzing

Run with nightly Rust and cargo-fuzz:

```sh
python3 fuzz/seed_corpus.py
cargo +nightly fuzz run protocol -- -max_len=65536 -timeout=2 -rss_limit_mb=1024 -max_total_time=120
```

The harness uses in-memory I/O, a fresh current-thread runtime per input, a
100 ms operation deadline and at most 64 events. No network or desktop is used.
libFuzzer aborts on panics, including panics in spawned decoder tasks.

The first input byte selects arbitrary handshake bytes (0 modulo 3), messages
after a valid handshake (1), or a bounded rectangle with mutated tile data (2).
For rectangles the next three bytes select a codec and dimensions; ZRLE payloads
are compressed by the harness so mutation reaches palette and run decoding.
The message mode also accepts arbitrary compressed bytes and rectangle headers.
This is bounded fuzz coverage, not a protocol-conformance or security audit.

In environments where LeakSanitizer cannot inspect threads, set
`ASAN_OPTIONS=detect_leaks=0` for the command. AddressSanitizer remains enabled;
that run does not check leaks.
