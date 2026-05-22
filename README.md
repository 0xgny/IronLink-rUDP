# IronLink rUDP: Reliable User-Space Transport Protocol

**IronLink rUDP** is a custom Reliable UDP transport protocol written in Rust. It serves as a user-space implementation of the core reliability mechanisms required by high-performance network interconnects (similar to the foundational concepts in NVIDIA/Mellanox RoCE). 

By wrapping inherently stateless and unreliable UDP sockets in a stateful layer, IronLink guarantees exact packet ordering and assured delivery without the heavy connection overhead of TCP.

---

## Core Features

* **Stop-and-Wait ARQ:** Ensures strict sequence ordering by requiring explicit acknowledgments (ACKs) before advancing the transmission window.
* **Dynamic RTT Measurement (Jacobson/Karels):** Replaces static timeouts by calculating a Smoothed Round Trip Time (SRTT) and RTT Variance (RTTVAR) to dynamically adjust the Retransmission Timeout (RTO) to current network conditions.
* **Karn's Algorithm:** Mitigates the "Retransmission Ambiguity" problem by isolating RTT calculations strictly to packets acknowledged on their first transmission attempt.
* **Exponential Backoff:** Automatically doubles the timeout window during network congestion events to prevent flood-induced packet loss.
* **Built-in Telemetry:** Tracks bytes sent, throughput, packet rates, and retransmission overhead in real-time.

---

## Protocol Architecture

### 1. The Packet Format
IronLink uses a minimal 5-byte header to maximize payload efficiency:

| Field | Size | Type | Description |
|---|---|---|---|
| **Seq Num** | 4 Bytes | `u32` | The sequence number of the packet. |
| **Flag** | 1 Byte | `u8` | `0x01` for DATA, `0x02` for ACK. |
| **Payload** | Variable | `[u8]` | The application data. |

### 2. The Transmission State Machine
* **Sender:** Wraps payload in a DATA packet, assigns a sequence number, and fires it over the UDP socket. It calculates an optimal timeout (RTO) and blocks. If an ACK with the matching sequence number arrives, it advances. If the timer expires, it triggers Exponential Backoff and retransmits.
* **Receiver:** Listens asynchronously, masking `EWOULDBLOCK` timeout errors. Upon receiving a DATA packet, it immediately returns an ACK. If the sequence number matches the expected state, the payload is passed to the application. If it is a duplicate (indicating a lost ACK), the payload is silently dropped but the ACK is re-transmitted.

---

## Project Structure

```text
ironlink_rudp/
├── Cargo.toml          # Crate metadata and dependencies
├── README.md           # This documentation
└── src/
    ├── lib.rs          # Module exports
    ├── packet.rs       # Byte-level serialization and struct definitions
    ├── socket.rs       # Core RUdpSocket, telemetry, and ARQ implementation
    └── bin/
        ├── client.rs   # Benchmark client pushing 5MB of payload
        └── server.rs   # Listener processing reliable streams

---

## How to run it locally

fork and clone the repo
open a terminal window and run: `cargo run --bin server`
open another and run `cargo run --bin client`
