use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};
use crate::packet::{Packet, PacketType};

/// Largest datagram we will put on the wire or read off it.
pub const MAX_PACKET_SIZE: usize = 2048;
/// [SeqNum (4)] [Type (1)]
pub const HEADER_SIZE: usize = 5;
/// Largest application payload that fits in a single packet.
pub const MAX_PAYLOAD_SIZE: usize = MAX_PACKET_SIZE - HEADER_SIZE;

/// Retransmission Timeout used before we have measured a single round trip.
pub const INITIAL_RTO: Duration = Duration::from_millis(500);
/// Floor and ceiling for the computed RTO, in milliseconds.
const MIN_RTO_MS: f64 = 50.0;
const MAX_RTO_MS: f64 = 2000.0;
/// How many times a single packet is put on the wire before the send gives up.
/// One original transmission plus nine retransmissions.
pub const DEFAULT_MAX_TRANSMISSION_ATTEMPTS: u32 = 10;

#[derive(Debug)]
pub struct SocketStats {
    pub packets_sent: usize,
    pub retransmissions: usize,
    pub total_bytes_sent: usize,
    /// When this socket was opened. Everything below is measured from here.
    started: Instant,
}

impl Default for SocketStats {
    fn default() -> Self {
        SocketStats {
            packets_sent: 0,
            retransmissions: 0,
            total_bytes_sent: 0,
            started: Instant::now(),
        }
    }
}

impl SocketStats {
    /// Time since the socket was opened.
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Live throughput in megabytes per second, including header bytes and
    /// retransmissions, i.e. what actually went on the wire.
    pub fn throughput_mbps(&self) -> f64 {
        let seconds = self.elapsed().as_secs_f64();
        if seconds <= 0.0 {
            return 0.0;
        }
        (self.total_bytes_sent as f64 / 1_048_576.0) / seconds
    }

    /// Live rate of unique (non-retransmitted) packets per second.
    pub fn packet_rate(&self) -> f64 {
        let seconds = self.elapsed().as_secs_f64();
        if seconds <= 0.0 {
            return 0.0;
        }
        self.packets_sent as f64 / seconds
    }

    /// Retransmissions as a percentage of unique packets sent.
    pub fn retransmission_overhead_pct(&self) -> f64 {
        if self.packets_sent == 0 {
            return 0.0;
        }
        (self.retransmissions as f64 / self.packets_sent as f64) * 100.0
    }

    /// Total megabytes put on the wire.
    pub fn total_mb(&self) -> f64 {
        self.total_bytes_sent as f64 / 1_048_576.0
    }
}

/// Everything the protocol tracks about one remote address.
///
/// Sequence numbers and RTT estimates are meaningless across peers: two clients
/// both open at sequence 0, and a peer on the far side of the world has nothing
/// to say about the RTT to a peer on localhost. So each peer gets its own copy.
#[derive(Debug)]
struct PeerState {
    /// Next sequence number we will send to this peer.
    next_seq_num: u32,
    /// Next sequence number we expect to receive from this peer.
    expected_seq_num: u32,

    // Dynamic Timeout Tracking (Jacobson/Karels)
    srtt: f64,
    rttvar: f64,
    rto: Duration,
}

impl Default for PeerState {
    fn default() -> Self {
        PeerState {
            next_seq_num: 0,
            expected_seq_num: 0,
            srtt: 0.0,
            rttvar: 0.0,
            rto: INITIAL_RTO,
        }
    }
}

impl PeerState {
    /// Recalculates the Retransmission Timeout from a fresh RTT sample using
    /// Jacobson/Karels math. Returns the new RTO.
    fn update_rto(&mut self, measured_rtt: Duration) -> Duration {
        let rtt_ms = measured_rtt.as_secs_f64() * 1000.0;

        if self.srtt == 0.0 {
            // First measurement initialization
            self.srtt = rtt_ms;
            self.rttvar = rtt_ms / 2.0;
        } else {
            // Standard TCP weights for smoothing
            let alpha = 0.125; // 1/8
            let beta = 0.25;   // 1/4

            self.rttvar = (1.0 - beta) * self.rttvar + beta * (self.srtt - rtt_ms).abs();
            self.srtt = (1.0 - alpha) * self.srtt + alpha * rtt_ms;
        }

        // RTO = Smoothed RTT + (4 * Variance), clamped to a sane band.
        let new_rto = (self.srtt + (4.0 * self.rttvar)).clamp(MIN_RTO_MS, MAX_RTO_MS);
        self.rto = Duration::from_millis(new_rto as u64);
        self.rto
    }

    /// Exponential backoff: each successive timeout doubles the wait, capped.
    /// Per Karn's Algorithm the backed-off value stays in force until a fresh,
    /// unambiguous RTT sample resets it in `update_rto`.
    fn back_off(&mut self) -> Duration {
        self.rto = self
            .rto
            .mul_f32(2.0)
            .min(Duration::from_millis(MAX_RTO_MS as u64));
        self.rto
    }
}

pub struct RUdpSocket {
    socket: UdpSocket,
    /// Per-remote-address protocol state. Keyed by peer address so that several
    /// peers can talk to this socket without corrupting each other's streams.
    peers: HashMap<SocketAddr, PeerState>,
    max_transmission_attempts: u32,
    pub stats: SocketStats,
}

impl RUdpSocket {
    pub fn bind(addr: &str) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(addr)?;
        socket.set_read_timeout(Some(INITIAL_RTO))?;

        Ok(RUdpSocket {
            socket,
            peers: HashMap::new(),
            max_transmission_attempts: DEFAULT_MAX_TRANSMISSION_ATTEMPTS,
            stats: SocketStats::default(),
        })
    }

    /// The local address this socket is bound to.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// How many times one packet is transmitted before `send_reliable` gives up.
    /// Must be at least 1.
    pub fn set_max_transmission_attempts(&mut self, attempts: u32) {
        self.max_transmission_attempts = attempts.max(1);
    }

    /// The current Retransmission Timeout for a given peer, if we have talked to it.
    pub fn rto_for(&self, peer: SocketAddr) -> Option<Duration> {
        self.peers.get(&peer).map(|p| p.rto)
    }

    /// Sends one payload and blocks until the peer acknowledges it.
    ///
    /// Returns `InvalidInput` if the payload cannot fit in a single packet, and
    /// `TimedOut` if the peer never acknowledged after the attempt limit.
    pub fn send_reliable(&mut self, payload: &[u8], target: &str) -> std::io::Result<()> {
        let target_addr = target
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("could not resolve target address '{}'", target),
                )
            })?;
        self.send_reliable_to(payload, target_addr)
    }

    /// Same as `send_reliable`, for an already-resolved address.
    pub fn send_reliable_to(&mut self, payload: &[u8], target: SocketAddr) -> std::io::Result<()> {
        if payload.len() > MAX_PAYLOAD_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "payload of {} bytes exceeds the {} byte limit for a single packet",
                    payload.len(),
                    MAX_PAYLOAD_SIZE
                ),
            ));
        }

        let peer = self.peers.entry(target).or_default();
        let seq_num = peer.next_seq_num;
        let mut rto = peer.rto;

        let packet = Packet {
            seq_num,
            p_type: PacketType::Data,
            payload: payload.to_vec(),
        };
        let data = packet.to_bytes();

        let mut buf = [0; MAX_PACKET_SIZE];
        let mut attempts: u32 = 0;
        let start_time = Instant::now();

        loop {
            // 1. Transmit Data
            self.socket.send_to(&data, target)?;
            self.stats.total_bytes_sent += data.len();
            attempts += 1;

            if attempts > 1 {
                self.stats.retransmissions += 1;
            } else {
                self.stats.packets_sent += 1;
            }

            // 2. Wait for the matching ACK until this attempt's deadline. Traffic
            //    from anyone else does not reset the clock and does not trigger a
            //    retransmission; it is simply skipped.
            let deadline = Instant::now() + rto;
            let timed_out = loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break true;
                }
                self.socket.set_read_timeout(Some(remaining))?;

                match self.socket.recv_from(&mut buf) {
                    Ok((amt, src)) => {
                        // Only the peer we sent to can acknowledge this packet.
                        if src != target {
                            continue;
                        }
                        if let Some(ack) = Packet::from_bytes(&buf[..amt]) {
                            if ack.p_type == PacketType::Ack && ack.seq_num == seq_num {
                                let peer = self.peers.get_mut(&target).expect("peer inserted above");

                                // Karn's Algorithm: only sample the RTT when the packet
                                // was never retransmitted, so we can be sure which
                                // transmission this ACK belongs to.
                                if attempts == 1 {
                                    peer.update_rto(start_time.elapsed());
                                }

                                peer.next_seq_num += 1;
                                return Ok(());
                            }
                        }
                        // Stale ACK or unrelated packet: keep waiting on this deadline.
                    }
                    Err(e) => {
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut
                        {
                            break true;
                        }
                        return Err(e);
                    }
                }
            };

            // 3. Timed out. Give up if we have exhausted our attempts, else back off.
            if timed_out {
                if attempts >= self.max_transmission_attempts {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!(
                            "peer {} did not acknowledge sequence {} after {} attempts",
                            target, seq_num, attempts
                        ),
                    ));
                }
                let peer = self.peers.get_mut(&target).expect("peer inserted above");
                rto = peer.back_off();
            }
        }
    }

    /// Blocks until the next in-order payload arrives from any peer.
    pub fn recv_reliable(&mut self) -> std::io::Result<(Vec<u8>, SocketAddr)> {
        let mut buf = [0; MAX_PACKET_SIZE];
        loop {
            // 1. Receive data, gracefully ignoring read timeouts
            let (amt, src) = match self.socket.recv_from(&mut buf) {
                Ok(result) => result,
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut
                    {
                        continue;
                    }
                    return Err(e);
                }
            };

            if let Some(packet) = Packet::from_bytes(&buf[..amt]) {
                if packet.p_type == PacketType::Data {
                    // 2. Acknowledge the packet, duplicate or not. A duplicate means
                    //    our previous ACK was lost, so it needs re-sending.
                    let ack = Packet {
                        seq_num: packet.seq_num,
                        p_type: PacketType::Ack,
                        payload: vec![],
                    };
                    self.socket.send_to(&ack.to_bytes(), src)?;

                    // 3. Deliver only if this is the sequence we expect from *this* peer.
                    let peer = self.peers.entry(src).or_default();
                    if packet.seq_num == peer.expected_seq_num {
                        peer.expected_seq_num += 1;
                        return Ok((packet.payload, src));
                    }
                    // Duplicates are silently ignored but acknowledged above.
                }
            }
        }
    }
}
