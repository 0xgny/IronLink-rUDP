# rUDP: High-Performance Network Protocol Implementation

This project; **(which I named IronLink-rUDP for some reason)** is a custom Reliable UDP transport protocol written in Rust. It serves as a user-space implementation of the core reliability mechanisms required by high-performance network interconnects (similar to the foundational concepts in NVIDIA/Mellanox RoCE). 

By wrapping inherently stateless and unreliable UDP sockets in a stateful layer, IronLink guarantees exact packet ordering and assured delivery without the heavy connection overhead of TCP.

---

## Core Features

* **Stop-and-Wait ARQ:** Ensures strict sequence ordering by requiring explicit acknowledgments (ACKs) before advancing the transmission window.
* **Dynamic RTT Measurement (Jacobson/Karels):** Replaces static timeouts by calculating a Smoothed Round Trip Time (SRTT) and RTT Variance (RTTVAR) to dynamically adjust the Retransmission Timeout (RTO) to current network conditions.
* **Karn's Algorithm:** Mitigates the "Retransmission Ambiguity" problem by isolating RTT calculations strictly to packets acknowledged on their first transmission attempt.
* **Exponential Backoff:** Each successive timeout doubles the retransmission window (500ms -> 1s -> 2s, capped) to prevent flood-induced packet loss. Per Karn's Algorithm the backed-off value stays in force until a fresh, unambiguous RTT sample resets it.
* **Built-in Telemetry:** Live counters on the socket expose throughput, packet rate, and retransmission overhead at any point during a transfer, not just at the end.
* **Per-Peer State:** Sequence numbers and RTT estimates are tracked per remote address, so multiple clients can share one socket without corrupting each other's streams. ACKs are only accepted from the address the packet was sent to.
* **Bounded Retries:** A packet is transmitted at most `DEFAULT_MAX_TRANSMISSION_ATTEMPTS` (10) times before `send_reliable` returns `TimedOut`, so an unreachable peer surfaces an error instead of hanging.

---

> **New here?** [`design/design-doc.md`](design/design-doc.md) explains the whole project from first principles, assuming no Rust and no networking background.

## Protocol Architecture

### 1. The Packet Format
IronLink uses a minimal 5-byte header to maximize payload efficiency:

| Field | Size | Type | Description |
|---|---|---|---|
| **Seq Num** | 4 Bytes | `u32` | The sequence number of the packet. |
| **Flag** | 1 Byte | `u8` | `0x01` for DATA, `0x02` for ACK. |
| **Payload** | Variable | `[u8]` | The application data. |

Datagrams are capped at `MAX_PACKET_SIZE` (2048 bytes), giving a `MAX_PAYLOAD_SIZE` of 2043 bytes per packet. `send_reliable` returns `InvalidInput` for anything larger rather than letting the kernel silently truncate it — there is no fragmentation layer.

### 2. The Transmission State Machine
* **Sender:** Wraps payload in a DATA packet, assigns a sequence number, and fires it over the UDP socket. It calculates an optimal timeout (RTO) and blocks. If an ACK with the matching sequence number arrives, it advances. If the timer expires, it triggers Exponential Backoff and retransmits.
* **Receiver:** Listens asynchronously, masking `EWOULDBLOCK` timeout errors. Upon receiving a DATA packet, it immediately returns an ACK. If the sequence number matches the expected state, the payload is passed to the application. If it is a duplicate (indicating a lost ACK), the payload is silently dropped but the ACK is re-transmitted.

---

## Project Structure

```text
rUDP/
├── Cargo.toml          # Crate metadata and dependencies
├── README.md           # This documentation
└── src/
    ├── lib.rs          # Module exports
    ├── packet.rs       # Byte-level serialization and struct definitions
    ├── socket.rs       # Core RUdpSocket, telemetry, and ARQ implementation
    └── bin/
        ├── client.rs   # Benchmark client pushing 5MB of payload
        └── server.rs   # Listener processing reliable streams
├── design/
│   └── design-doc.md   # Full walkthrough for newcomers
└── tests/
    └── integration.rs  # Round-trip, payload limits, backoff timing, multi-client, retry cap
```
---

## How to run it locally

* fork and clone the repo
* open a terminal window and run: `cargo run --release --bin server`
* open another and run `cargo run --release --bin client`
* run the test suite with `cargo test` (6 tests, ~4s)

Use `--release` for the benchmark; an unoptimized build reports several times lower throughput.
