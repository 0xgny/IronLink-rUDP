use ironlink_rudp::socket::{RUdpSocket, MAX_PAYLOAD_SIZE};
use std::thread;

/// A full-size payload must arrive byte-for-byte identical, not truncated by the recv buffer.
#[test]
fn max_size_payload_survives_the_round_trip() {
    let mut server = RUdpSocket::bind("127.0.0.1:9101").expect("bind server");
    let payload: Vec<u8> = (0..MAX_PAYLOAD_SIZE).map(|i| (i % 251) as u8).collect();
    let expected = payload.clone();

    let receiver = thread::spawn(move || {
        let (data, _src) = server.recv_reliable().expect("recv");
        data
    });

    let mut client = RUdpSocket::bind("127.0.0.1:0").expect("bind client");
    client.send_reliable(&payload, "127.0.0.1:9101").expect("send");

    let received = receiver.join().expect("receiver thread");
    assert_eq!(received.len(), MAX_PAYLOAD_SIZE);
    assert_eq!(received, expected);
}

/// Payloads that cannot fit in one datagram are rejected rather than silently clipped.
#[test]
fn oversized_payload_is_rejected() {
    let mut client = RUdpSocket::bind("127.0.0.1:0").expect("bind client");
    let payload = vec![0u8; MAX_PAYLOAD_SIZE + 1];
    let err = client
        .send_reliable(&payload, "127.0.0.1:9102")
        .expect_err("oversized payload should error");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}

/// Successive timeouts must compound (500ms -> 1s -> 2s), not repeat at a fixed 2x
/// of the base RTO. A raw socket swallows the first datagrams and timestamps the
/// retransmits, then finally ACKs to release the sender.
#[test]
fn retransmission_backoff_is_exponential() {
    use std::net::UdpSocket;
    use std::time::Instant;
    use ironlink_rudp::packet::{Packet, PacketType};

    let sink = UdpSocket::bind("127.0.0.1:9103").expect("bind sink");
    let gaps = thread::spawn(move || {
        let mut buf = [0u8; 2048];
        let mut arrivals = Vec::new();
        let mut peer = None;
        for _ in 0..4 {
            let (_amt, src) = sink.recv_from(&mut buf).expect("sink recv");
            arrivals.push(Instant::now());
            peer = Some(src);
        }
        // Release the sender so it does not spin forever.
        let ack = Packet { seq_num: 0, p_type: PacketType::Ack, payload: vec![] };
        sink.send_to(&ack.to_bytes(), peer.unwrap()).expect("ack");
        arrivals
            .windows(2)
            .map(|w| w[1].duration_since(w[0]).as_millis())
            .collect::<Vec<_>>()
    });

    let mut client = RUdpSocket::bind("127.0.0.1:0").expect("bind client");
    client.send_reliable(b"hello", "127.0.0.1:9103").expect("send");

    let gaps = gaps.join().expect("sink thread");
    println!("retransmit gaps (ms): {:?}", gaps);
    let within = |actual: u128, target: u128| actual.abs_diff(target) < 150;
    assert!(within(gaps[0], 500), "first gap should be ~500ms, got {}", gaps[0]);
    assert!(within(gaps[1], 1000), "second gap should be ~1000ms, got {}", gaps[1]);
    assert!(within(gaps[2], 2000), "third gap should be ~2000ms, got {}", gaps[2]);
    assert_eq!(client.stats.retransmissions, 3);
}

/// Two clients each open at sequence 0. A single global counter on the receiver
/// would drop the second client's traffic; per-peer state keeps both streams whole.
#[test]
fn two_clients_do_not_corrupt_each_others_streams() {
    let mut server = RUdpSocket::bind("127.0.0.1:9104").expect("bind server");

    let receiver = thread::spawn(move || {
        let mut received = Vec::new();
        for _ in 0..4 {
            let (data, _src) = server.recv_reliable().expect("recv");
            received.push(String::from_utf8(data).expect("utf8"));
        }
        received.sort();
        received
    });

    for name in ["alice", "bob"] {
        let mut client = RUdpSocket::bind("127.0.0.1:0").expect("bind client");
        for n in 0..2 {
            client
                .send_reliable(format!("{}-{}", name, n).as_bytes(), "127.0.0.1:9104")
                .expect("send");
        }
    }

    let received = receiver.join().expect("receiver thread");
    assert_eq!(received, vec!["alice-0", "alice-1", "bob-0", "bob-1"]);
}

/// A peer that never answers must surface an error instead of looping forever.
#[test]
fn unreachable_peer_gives_up_with_timed_out() {
    let mut client = RUdpSocket::bind("127.0.0.1:0").expect("bind client");
    client.set_max_transmission_attempts(3);

    let err = client
        .send_reliable(b"anyone there?", "127.0.0.1:9105")
        .expect_err("should give up");

    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    assert_eq!(client.stats.packets_sent, 1);
    assert_eq!(client.stats.retransmissions, 2);
}

/// An ACK from an address we did not send to must not satisfy the send.
#[test]
fn ack_from_a_third_party_is_ignored() {
    use std::net::UdpSocket;
    use ironlink_rudp::packet::{Packet, PacketType};

    // The real target never answers. A different socket forges ACKs for seq 0.
    let _silent_target = UdpSocket::bind("127.0.0.1:9106").expect("bind target");
    let mut client = RUdpSocket::bind("127.0.0.1:0").expect("bind client");
    client.set_max_transmission_attempts(2);
    let client_addr = client.local_addr().expect("local addr");

    let spoofer = UdpSocket::bind("127.0.0.1:9107").expect("bind spoofer");
    let forger = thread::spawn(move || {
        let ack = Packet { seq_num: 0, p_type: PacketType::Ack, payload: vec![] };
        for _ in 0..20 {
            let _ = spoofer.send_to(&ack.to_bytes(), client_addr);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });

    let err = client
        .send_reliable(b"payload", "127.0.0.1:9106")
        .expect_err("forged ACKs must not satisfy the send");
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);

    let _ = forger.join();
}
