use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};
use crate::packet::{Packet, PacketType};

#[derive(Default, Debug)]
pub struct SocketStats {
    pub packets_sent: usize,
    pub retransmissions: usize,
    pub total_bytes_sent: usize,
}

pub struct RUdpSocket {
    socket: UdpSocket,
    next_seq_num: u32,
    expected_seq_num: u32,
    pub stats: SocketStats,

    // Dynamic Timeout Tracking (Jacobson/Karels)
    srtt: f64,
    rttvar: f64,
    rto: Duration,
}

impl RUdpSocket {
    pub fn bind(addr: &str) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(addr)?;

        // Start with a safe default Retransmission Timeout (RTO) of 500ms
        let initial_rto = Duration::from_millis(500);
        socket.set_read_timeout(Some(initial_rto))?;

        Ok(RUdpSocket {
            socket,
            next_seq_num: 0,
            expected_seq_num: 0,
            stats: SocketStats::default(),
            srtt: 0.0,
            rttvar: 0.0,
            rto: initial_rto,
        })
    }

    /// Calculates and applies the new Retransmission Timeout (RTO) based on Jacobson/Karels math
    fn update_rto(&mut self, measured_rtt: Duration) {
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

        // RTO = Smoothed RTT + (4 * Variance)
        let mut new_rto = self.srtt + (4.0 * self.rttvar);

        // Clamp the RTO so it never drops below 50ms or exceeds 2000ms
        new_rto = new_rto.clamp(50.0, 2000.0);

        self.rto = Duration::from_millis(new_rto as u64);
        let _ = self.socket.set_read_timeout(Some(self.rto));
    }

    pub fn send_reliable(&mut self, payload: &[u8], target: &str) -> std::io::Result<()> {
        let packet = Packet {
            seq_num: self.next_seq_num,
            p_type: PacketType::Data,
            payload: payload.to_vec(),
        };
        let data = packet.to_bytes();

        let mut attempts = 0;
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

            // 2. Wait for ACK
            let mut buf = [0; 1024];
            match self.socket.recv_from(&mut buf) {
                Ok((amt, _src)) => {
                    if let Some(ack_packet) = Packet::from_bytes(&buf[..amt]) {
                        if ack_packet.p_type == PacketType::Ack && ack_packet.seq_num == self.next_seq_num {

                            // Karn's Algorithm: Only update RTO if the packet was never retransmitted.
                            // This avoids the "Retransmission Ambiguity" problem.
                            if attempts == 1 {
                                self.update_rto(start_time.elapsed());
                            }

                            self.next_seq_num += 1;
                            return Ok(());
                        }
                    }
                }
                Err(e) => {
                    // 3. Handle Timeouts & Exponential Backoff
                    if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut {
                        // Double the RTO to ease congestion, capped at 2 seconds
                        let backed_off_rto = self.rto.mul_f32(2.0).min(Duration::from_millis(2000));
                        let _ = self.socket.set_read_timeout(Some(backed_off_rto));
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    }

    pub fn recv_reliable(&mut self) -> std::io::Result<(Vec<u8>, SocketAddr)> {
        let mut buf = [0; 1024];
        loop {
            // 1. Receive data, gracefully ignoring timeouts
            let (amt, src) = match self.socket.recv_from(&mut buf) {
                Ok(result) => result,
                Err(e) => {
                    // Silently loop if the read timeout expires while waiting for data
                    if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut {
                        continue;
                    }
                    return Err(e);
                }
            };

            if let Some(packet) = Packet::from_bytes(&buf[..amt]) {
                if packet.p_type == PacketType::Data {

                    // 2. Acknowledge the packet
                    let ack = Packet {
                        seq_num: packet.seq_num,
                        p_type: PacketType::Ack,
                        payload: vec![],
                    };
                    self.socket.send_to(&ack.to_bytes(), src)?;

                    // 3. Process payload only if it's the sequence we expect
                    if packet.seq_num == self.expected_seq_num {
                        self.expected_seq_num += 1;
                        return Ok((packet.payload, src));
                    }
                    // Duplicates are silently ignored but acknowledged above
                }
            }
        }
    }
}
