# OpenISO

Open-source isometric training app using the [Tindeq Progressor](https://tindeq.com/) force sensor.

Early stage. Currently connects to the Progressor over BLE and streams live force data.

## Status

- [x] BLE scan, connect, and subscribe to Tindeq Progressor
- [x] Parse weight + timestamp from notification stream
- [ ] Compute MVC from raw data
- [ ] Session recording and history
- [ ] Visualization (TUI / Web UI)?

## Requirements

- Rust (2024 edition)
- A BLE adapter
- Tindeq Progressor

## Usage

```bash
cargo run
```

Scans for a Progressor, connects, tares, and prints live force readings.

## License

MIT
