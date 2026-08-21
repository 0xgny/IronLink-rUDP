# IronLink rUDP — Design Document

*Written for someone who has never seen this repository, and does not need to know Rust.*

---

## 1. What this project is, in one paragraph

When two computers send data over the internet, they need to agree on rules: how the
data is chopped up, how the receiver says "got it", and what happens when a piece
goes missing. Those rules are called a **transport protocol**. This project builds
one from scratch. It takes the internet's simplest, most careless delivery
mechanism (UDP) and wraps it in a layer of bookkeeping that turns it into something
dependable: every piece of data arrives, exactly once, in the order it was sent.
It is about 470 lines of Rust, including the two demo programs, with **no external libraries at all**.

---

## 2. The problem this solves

### The two ways computers already send data

Imagine you want to mail a 500-page book to a friend, one page per envelope.

**UDP is the careless postal service.** You drop 500 envelopes in the mailbox. They
are cheap and fast. But the post office makes no promises: some envelopes get lost,
some arrive out of order, some arrive twice. Nobody tells you which. Your friend
receives a jumbled, incomplete pile and has no idea what's missing.

**TCP is the expensive courier.** It guarantees every page arrives in order. But
before you can send anything you must arrange a formal contract (a "handshake"),
and the courier does a lot of work on your behalf that you may not want — pacing
itself when it thinks the roads are busy, reordering things in a buffer, keeping
connections alive. All of that costs time and memory.

**This project builds a third option.** It uses the careless postal service, but
adds its own tracking system on top: number every envelope, wait for your friend to
confirm each one, and re-send anything that doesn't get confirmed. You get the
reliability of the courier, but you control exactly how it behaves, and you only
pay for the guarantees you actually asked for.

### Why anyone would want this

Very fast networking hardware — the kind used inside AI training clusters and
datacenters, such as NVIDIA/Mellanox's RoCE ("RDMA over Converged Ethernet") — does
exactly this trick. It runs a reliability layer directly on top of raw packets
rather than using TCP, because TCP's extra machinery gets in the way when you have
a fast, well-behaved private network.

That hardware implements the logic in silicon. This project implements the same
*category* of logic in software, so you can read it, run it, and watch it work. It
is a learning vehicle for the state machines and timing math behind reliable
transport — not a drop-in replacement for RoCE, which uses a considerably more
aggressive design (see [Section 12](#12-what-this-does-not-do)).

---

## 3. Libraries used: none

This is worth stating clearly because it's unusual. Open `Cargo.toml` — the file
that lists a Rust project's dependencies — and the dependency list is empty:

```toml
[package]
name = "ironlink_rudp"
version = "0.1.0"
edition = "2024"

[dependencies]
```

Everything is built on Rust's **standard library** (`std`), which ships with the
language itself. The four pieces used are:

| What's used | What it does |
|---|---|
| `std::net::UdpSocket` | The raw, unreliable mailbox. Send bytes to an address; receive bytes from anyone. Nothing more. |
| `std::time::Instant` / `Duration` | A stopwatch and a length of time. Used to measure round trips and to set deadlines. |
| `std::collections::HashMap` | A lookup table. Used to store per-conversation bookkeeping, keyed by who we're talking to. |
| `std::io::Result` | Rust's standard way of saying "this operation might fail" — the caller must handle the failure. |

Every reliability mechanism described below is written by hand on top of those.
Nothing is delegated to a framework.

---

## 4. The packet format

Every message on the wire is a blob of bytes. The first 5 bytes are the **header**
(our bookkeeping), and everything after is the **payload** (the actual data).

```
 byte:  0    1    2    3      4       5 ...
      +----+----+----+----+--------+-------------------+
      |  sequence number  |  type  |     payload       |
      |     (4 bytes)     | (1 byte)|   (0-2043 bytes) |
      +----+----+----+----+--------+-------------------+
```

- **Sequence number** — a counter. The first message is 0, the next is 1, and so on.
  This is how the receiver knows the order, and how it spots duplicates. Stored
  "big-endian", meaning the most significant byte comes first — the standard
  convention for network data, so that machines with different internal byte
  orderings agree.
- **Type** — `0x01` means DATA (real content), `0x02` means ACK (short for
  "acknowledgement" — a receipt saying "I got number N").
- **Payload** — whatever the application wanted to send.

**Why only 5 bytes matters:** TCP's header is 20 bytes minimum, often more. Every
header byte is bandwidth spent on bookkeeping instead of data. At 5 bytes on a
1024-byte message, overhead is under half a percent.

**Size limit.** A whole packet is capped at 2048 bytes, leaving 2043 bytes of
payload. There is no fragmentation layer — nothing splits a large message across
several packets. If you hand it something bigger, it refuses with a clear error
rather than silently cutting off the end. (This is enforced in `send_reliable_to`;
splitting large messages is the caller's job.)

The code for this section is `src/packet.rs`: `to_bytes` builds the blob,
`from_bytes` takes a blob apart and returns nothing if it's too short to be valid.

---

## 5. The core rule: Stop-and-Wait

This is the heart of the protocol, and it is deliberately the simplest scheme that
works.

> **Send one message. Do not send anything else until the receiver confirms it.**

Concretely:

**The sender** (`send_reliable` in `src/socket.rs`):

1. Wrap the payload with the next sequence number and mark it DATA.
2. Send it.
3. Start a countdown and wait for an ACK carrying that same sequence number.
4. **If the right ACK arrives** → increment the sequence number, done, return
   success to the application.
5. **If the countdown expires** → assume it was lost, wait longer next time
   (Section 7), and send the exact same bytes again.
6. **If it has been re-sent too many times** → give up and report an error.

**The receiver** (`recv_reliable` in `src/socket.rs`):

1. Wait for a packet.
2. When a DATA packet arrives, **immediately send back an ACK** with that same
   sequence number.
3. Then check the number:
   - **Is it the one we were expecting?** Hand the payload to the application and
     start expecting the next number.
   - **Is it one we already handled?** Throw the payload away — but note that we
     already sent the ACK in step 2. This is the important part.

### Why the receiver ACKs duplicates

Two different things can go wrong, and from the sender's point of view they look
identical: the DATA might have been lost, *or* the DATA arrived fine and the ACK
coming back was lost. Either way the sender sees silence and re-sends.

If the original DATA actually arrived, the receiver now gets it a second time. If
it stayed quiet, the sender would re-send forever. So the receiver always ACKs —
even for data it is discarding — because the ACK is what unblocks the sender. It
just makes sure not to hand the same payload to the application twice. That is what
"exactly once" delivery means here.

### The trade-off

Stop-and-Wait is easy to reason about and impossible to get subtly wrong, but only
one message is ever in flight. The sender spends most of its time waiting. Real
high-performance protocols keep dozens or hundreds of messages in flight at once (a
"sliding window"). That is the single biggest performance difference between this
and a production protocol, and it is a deliberate scope choice, not an oversight.

---

## 6. Who am I talking to? Per-peer state

Several different computers can send to the same socket. Each one starts its own
counting at sequence 0.

If the receiver kept a single "next expected number" for everyone, then two clients
both sending their first message would both send number 0 — the receiver would
accept one and throw the other away as a duplicate. Data would vanish silently.

So all bookkeeping lives in a lookup table keyed by the remote address. Each peer
gets its own `PeerState` record holding:

- the next sequence number **we send** to that peer,
- the next sequence number **we expect** from that peer,
- that peer's own timing estimates (Section 7).

Timing is per-peer for the same reason: a machine on the far side of the world and
a machine on the same laptop have nothing useful to say about each other's speed.

Relatedly, the sender only accepts an ACK that came **from the exact address it
sent to**. An unrelated packet arriving mid-wait is skipped without disturbing the
countdown, rather than being mistaken for a receipt.

---

## 7. The hard part: how long should we wait?

Step 3 above says "start a countdown". Choosing that number is the genuinely
interesting problem, and three classic algorithms combine to solve it. The countdown
is called the **RTO** — Retransmission Timeout.

### Why a fixed number is wrong

Pick **too short**, and you re-send messages that were merely slow. Those duplicates
add traffic to a network that is already struggling, which makes it slower, which
causes more spurious re-sends. This is how networks collapse.

Pick **too long**, and every genuine loss stalls the entire conversation while you
sit there waiting for a receipt that will never come.

The right value depends on the network, and the network changes minute to minute.
So it has to be measured continuously.

### 7a. Jacobson/Karels: learning the network's speed

Each time a message is confirmed, we know how long the round trip took — the **RTT**
(Round Trip Time). But you cannot just use the last measurement as your timeout,
because a single unusually fast trip would set an unrealistically tight deadline.

The Jacobson/Karels algorithm (the same one TCP uses) keeps two running values:

- **SRTT** — "Smoothed RTT", a rolling average of how long trips take. Each new
  measurement nudges it by 12.5%, so it tracks real trends but ignores single blips.
- **RTTVAR** — "RTT Variance", a rolling measure of how *inconsistent* the times
  are. A steady network has low variance; an erratic one has high variance.

The timeout is then:

```
RTO = SRTT + 4 × RTTVAR
```

In plain terms: **wait for the typical round trip, plus a safety margin that grows
when the network is being unpredictable.** On a steady link the margin is small and
losses are detected quickly. On an erratic link the margin widens automatically, so
slow-but-alive messages aren't mistaken for lost ones.

The result is clamped between 50ms and 2000ms so a freak measurement can't produce
an absurd deadline in either direction.

### 7b. Karn's Algorithm: which measurements are trustworthy

Here is a subtle trap. Suppose we send message 7, hear nothing, re-send it, and then
an ACK arrives. How long did that round trip take?

We genuinely cannot tell. The ACK might be answering the *first* transmission
(arriving late) or the *second* (arriving promptly). The two give wildly different
answers, and there is nothing in the packet to distinguish them. This is called
**retransmission ambiguity**.

Karn's Algorithm resolves it with a rule of admirable bluntness: **only measure the
round trip for messages that were sent exactly once.** If a message had to be
re-sent, its ACK is used to unblock the sender but is never fed into the SRTT math.
An ambiguous number is worse than no number.

In the code this is the check `if attempts == 1`.

### 7c. Exponential Backoff: what to do when it's clearly bad

Karn's rule leaves a gap: if messages keep needing retransmission, we never take a
new measurement, so the timeout would be frozen at a value we already know is too
short. We would hammer a struggling network at a fixed rate.

The fix is to **double the timeout on every consecutive failure**:

| Attempt | Wait before re-sending |
|---|---|
| 1st retry | 500 ms |
| 2nd retry | 1000 ms |
| 3rd retry | 2000 ms |
| further retries | 2000 ms (capped) |

Each failure makes us back off further, so a struggling network gets exponentially
less traffic from us instead of a steady drumbeat. The backed-off value **stays in
force** until a clean, unambiguous measurement replaces it — that's the other half
of Karn's Algorithm, and it's what makes the two work as a pair.

The doubling is capped at 2 seconds so a dead peer is detected in reasonable time.

### 7d. Giving up

Retrying forever is not reliability — it's a hang. After a configurable number of
attempts (10 by default), the send stops and returns a "timed out" error naming the
peer and the sequence number. The application decides what to do next. The sequence
number is *not* advanced, so a retry resumes cleanly from the same point.

---

## 8. Telemetry

The socket counts three raw numbers as it works: unique packets sent,
retransmissions, and total bytes put on the wire. It also records when it was
opened.

From those, it derives live figures on demand — `throughput_mbps()`,
`packet_rate()`, `retransmission_overhead_pct()`, `total_mb()`, `elapsed()`. They
can be read at any moment while traffic is flowing, not just at the end, which is
how the benchmark client prints a progress line every 1000 packets.

The number worth watching is **retransmission overhead**: retransmissions as a
percentage of unique packets. On a healthy link it is 0%. If it starts climbing,
the network is dropping packets and throughput will fall.

---

## 9. Code map

```text
ironlink_rudp/
├── Cargo.toml              Project name, Rust edition, dependencies (none)
├── README.md               Short overview
├── design/
│   └── design-doc.md       This document
├── src/
│   ├── lib.rs              Two lines; makes the modules below public
│   ├── packet.rs           The 5-byte format: bytes <-> struct
│   ├── socket.rs           Everything else: the protocol, timing, per-peer state
│   └── bin/
│       ├── client.rs       Demo: sends 5 MB and reports live speed
│       └── server.rs       Demo: listens and tallies what arrives
└── tests/
    └── integration.rs      Six tests that run real sockets over loopback
```

Reading order for someone new: `packet.rs` (small and self-contained), then
`socket.rs` top to bottom, then `tests/integration.rs` to see the guarantees
stated as executable claims.

### The two important types in `socket.rs`

- **`PeerState`** — the bookkeeping for one remote address: two sequence counters
  and three timing values. It owns the timing math (`update_rto`, `back_off`).
- **`RUdpSocket`** — the thing you actually use. Holds the real UDP socket, the
  table of `PeerState` records, the retry limit, and the stats. It exposes:
  - `bind(address)` — open a socket on a local address
  - `send_reliable(payload, target)` — send, and don't return until confirmed
  - `recv_reliable()` — block until the next in-order payload arrives
  - `set_max_transmission_attempts(n)` — tune how long to persist
  - `stats` — the live telemetry described above

---

## 10. How to run it

### Step 1: Install Rust

If you don't have it, one command installs everything (from
[rustup.rs](https://rustup.rs)):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Check it worked:

```bash
rustc --version
```

This project uses Rust edition 2024, so you need **Rust 1.85 or newer**. If yours
is older, run `rustup update`.

### Step 2: Get the code and build it

```bash
git clone <this-repo-url>
cd rUDP
cargo build --release
```

`cargo` is Rust's build tool and package manager. `--release` turns on
optimizations — without it the benchmark numbers are several times lower.

### Step 3: Run the demo

You need **two terminal windows**, because this is a two-sided conversation.

In the first, start the receiver:

```bash
cargo run --release --bin server
```

You should see:

```
Server listening on 127.0.0.1:8080...
```

In the second, run the benchmark sender:

```bash
cargo run --release --bin client
```

`127.0.0.1` means "this same computer", so both programs talk over the machine's
internal loopback network. No real network or configuration is needed.

### What you should see

The client sends 5000 messages of 1 KB each (about 5 MB total) and prints progress
as it goes:

```
Starting Benchmark: Sending 5000 packets (1KB each)...
  [ 1000/5000]  24.36 MB/s |   24825 pkt/s | 0 retransmits (0.00%)
  [ 2000/5000]  33.20 MB/s |   33836 pkt/s | 0 retransmits (0.00%)
  ...
--- Benchmark Results ---
Time Elapsed:    0.11 seconds
Total Data Sent: 4.91 MB
Throughput:      44.07 MB/s
Packet Rate:     44911 packets/sec
Retransmissions: 0 (0.00% overhead)
```

And the server reports what arrived:

```
127.0.0.1:61316 -> 1000 packets, 0.98 MB received
...
127.0.0.1:61316 -> 5000 packets, 4.88 MB received
```

**How to read this.** Zero retransmissions is expected — loopback doesn't lose
packets. The interesting figure is the packet rate: roughly 45,000 per second means
each full round trip (send, receive, acknowledge, confirm) takes about 22
microseconds. That is the cost of Stop-and-Wait: the sender is idle for most of
that, waiting. Throughput climbs across the run because the timing estimates settle
and startup costs get amortized.

The server keeps running until you stop it with `Ctrl+C`.

### Step 4: Run the tests

```bash
cargo test
```

All six should pass. It takes about 4 seconds — three of those are one test
deliberately sitting through real timeouts to verify the backoff schedule.

---

## 11. Using it in your own program

The demos are just examples; the crate is a library. A minimal sender and receiver:

```rust
use ironlink_rudp::socket::RUdpSocket;

// Receiving side
let mut server = RUdpSocket::bind("127.0.0.1:8080")?;
let (payload, from) = server.recv_reliable()?;   // blocks until data arrives
println!("{} bytes from {}", payload.len(), from);

// Sending side
let mut client = RUdpSocket::bind("127.0.0.1:0")?;  // port 0 = pick any free port
client.send_reliable(b"hello", "127.0.0.1:8080")?;  // returns once confirmed
```

`send_reliable` returns only after the peer has confirmed receipt, or fails with an
error explaining why. There is nothing else to manage.

---

## 12. What this does *not* do

Stated plainly, because knowing a system's boundaries is as important as knowing its
features.

- **No sliding window.** One message in flight at a time (Section 5). This is the
  main gap between this and a production protocol, and the first thing to build next.
- **No congestion control.** It backs off on timeouts, but has no congestion window
  and no concept of a fair share of the link. It will not detect that it is
  overwhelming a network until packets actually start dropping.
- **No flow control.** A fast sender can outpace a slow receiver's ability to
  process data. Stop-and-Wait masks this in practice, but nothing enforces it.
- **No fragmentation.** Messages over 2043 bytes are rejected, not split.
- **No connection handshake or teardown.** State is created implicitly on first
  contact. There is no "hello" or "goodbye", so neither side can tell the difference
  between a peer that finished and a peer that vanished — only the retry limit
  eventually surfaces it.
- **No encryption or authentication.** Anyone who can reach the socket can send
  traffic to it. The address check on ACKs is a correctness measure, not a security
  one — addresses can be forged. Do not expose this to an untrusted network.
- **Receiver restarts lose data silently.** If the receiving process restarts, its
  expected sequence number resets to 0 while the sender continues from where it was.
  The receiver will acknowledge those higher-numbered packets and then discard them
  as unexpected. A connection handshake would fix this.
- **Sequence numbers do not wrap.** They stop making sense after about 4.3 billion
  messages to a single peer.
- **Peer records are never evicted.** A long-lived server that talks to many
  short-lived clients will accumulate one small record per client address, forever.

---

## 13. Glossary

| Term | Meaning |
|---|---|
| **UDP** | The internet's minimal delivery service. Fast, no guarantees. |
| **TCP** | The internet's reliable delivery service. Guarantees a lot, costs more. |
| **Packet / datagram** | One blob of bytes sent as a single unit. |
| **Header** | The bookkeeping bytes at the front of a packet. Here, 5 bytes. |
| **Payload** | The actual data being sent, after the header. |
| **ACK** | Acknowledgement. A receipt confirming a specific numbered packet arrived. |
| **Sequence number** | The counter that gives packets their order and identity. |
| **ARQ** | Automatic Repeat reQuest. Any scheme built on "re-send what wasn't confirmed". |
| **Stop-and-Wait** | The simplest ARQ: one packet in flight, wait for its ACK. |
| **RTT** | Round Trip Time. How long between sending and getting the receipt. |
| **RTO** | Retransmission Timeout. How long we wait before assuming loss. |
| **SRTT** | Smoothed RTT. The rolling average of measured round trips. |
| **RTTVAR** | RTT Variance. How inconsistent those round trips have been. |
| **Backoff** | Waiting progressively longer after each consecutive failure. |
| **Loopback / 127.0.0.1** | The address meaning "this same machine". |
| **Socket** | A program's endpoint for sending and receiving network data. |
| **RoCE** | RDMA over Converged Ethernet — datacenter hardware that does this in silicon. |
