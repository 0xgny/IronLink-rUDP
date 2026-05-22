use std::net::{UdpSocket, SocketAddr};
use std::time::Duration;
use crate::packet::{Packet, PacketType};

pub struct RUdpSocket {
    socket: UdpSocket,
    next_seq_num: u32,
    expected_seq_num: u32,
}

impl RUdpSocket {
    pub fn bind(addr: &str) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(addr)?;
        // Set a small read timeout for our ARQ retransmission logic
        socket.set_read_timeout(Some(Duration::from_millis(500)))?;

        Ok(RUdpSocket {
            socket,
            next_seq_num: 0,
            expected_seq_num: 0,
        })
    }

    pub fn send_reliable(&mut self, payload: &[u8], target: &str) -> std::io::Result<()> {
        let packet = Packet {
            seq_num: self.next_seq_num,
            p_type: PacketType::Data,
            payload: payload.to_vec(),
        };
        let data = packet.to_bytes();

        loop {
            // 1. Send the packet
            self.socket.send_to(&data, target)?;
            println!("-> Sent packet Seq: {}", self.next_seq_num);

            // 2. Wait for ACK
            let mut buf = [0; 1024];
            match self.socket.recv_from(&mut buf) {
                Ok((amt, _src)) => {
                    if let Some(ack_packet) = Packet::from_bytes(&buf[..amt]) {
                        if ack_packet.p_type == PacketType::Ack && ack_packet.seq_num == self.next_seq_num {
                            println!("<- Received ACK for Seq: {}", self.next_seq_num);
                            self.next_seq_num += 1;
                            return Ok(());
                        }
                    }
                }
                Err(e) => {
                    // Timeout occurred, loop will repeat and retransmit
                    if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut {
                        println!("-- Timeout! Retransmitting Seq: {}", self.next_seq_num);
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
            // 1. Receive data
            let (amt, src) = match self.socket.recv_from(&mut buf) {
                Ok(result) => result,
                Err(e) => {
                    // If the 500ms timeout triggers, just loop around and keep waiting
                    if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut {
                        continue;
                    }
                    // If it's a real error, bubble it up
                    return Err(e);
                }
            };

            if let Some(packet) = Packet::from_bytes(&buf[..amt]) {
                if packet.p_type == PacketType::Data {
                    // 2. Send ACK regardless of whether it's new or a duplicate
                    let ack = Packet {
                        seq_num: packet.seq_num,
                        p_type: PacketType::Ack,
                        payload: vec![],
                    };
                    self.socket.send_to(&ack.to_bytes(), src)?;
                    // println!("-> Sent ACK for Seq: {}", packet.seq_num); // Optional: comment out if too noisy

                    // 3. Process payload only if it's the sequence we expect
                    if packet.seq_num == self.expected_seq_num {
                        self.expected_seq_num += 1;
                        return Ok((packet.payload, src));
                    } else {
                        println!("-- Discarded duplicate packet Seq: {}", packet.seq_num);
                    }
                }
            }
        }
    }
}
