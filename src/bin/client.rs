use ironlink_rudp::socket::RUdpSocket;

fn main() {
    let mut client = RUdpSocket::bind("127.0.0.1:0").expect("Failed to bind");
    let server_addr = "127.0.0.1:8080";

    let payload = vec![0u8; 1024]; // 1 KB payload
    let num_packets = 5000; // Total 5 MB

    println!("Starting Benchmark: Sending {} packets (1KB each)...", num_packets);

    for i in 0..num_packets {
        client.send_reliable(&payload, server_addr).expect("Send failed");

        // Live telemetry, straight off the socket, every 1000 packets.
        if (i + 1) % 1000 == 0 {
            println!(
                "  [{:>5}/{}] {:>6.2} MB/s | {:>7.0} pkt/s | {} retransmits ({:.2}%)",
                i + 1,
                num_packets,
                client.stats.throughput_mbps(),
                client.stats.packet_rate(),
                client.stats.retransmissions,
                client.stats.retransmission_overhead_pct(),
            );
        }
    }

    println!("\n--- Benchmark Results ---");
    println!("Time Elapsed:    {:.2} seconds", client.stats.elapsed().as_secs_f64());
    println!("Total Data Sent: {:.2} MB", client.stats.total_mb());
    println!("Throughput:      {:.2} MB/s", client.stats.throughput_mbps());
    println!("Packet Rate:     {:.0} packets/sec", client.stats.packet_rate());
    println!(
        "Retransmissions: {} ({:.2}% overhead)",
        client.stats.retransmissions,
        client.stats.retransmission_overhead_pct()
    );
}
