use ironlink_rudp::socket::RUdpSocket;
use std::time::Instant;

fn main() {
    let mut client = RUdpSocket::bind("127.0.0.1:0").expect("Failed to bind");
    let server_addr = "127.0.0.1:8080";

    let payload = vec![0u8; 1024]; // 1 KB payload
    let num_packets = 5000; // Total 5 MB

    println!("Starting Benchmark: Sending {} packets (1KB each)...", num_packets);

    let start_time = Instant::now();

    for _ in 0..num_packets {
        client.send_reliable(&payload, server_addr).expect("Send failed");
    }

    let duration = start_time.elapsed();
    let seconds = duration.as_secs_f64();
    let total_mb = (client.stats.total_bytes_sent as f64) / 1_048_576.0;
    let throughput_mbps = total_mb / seconds;
    let packets_per_sec = (client.stats.packets_sent as f64) / seconds;
    let overhead_pct = (client.stats.retransmissions as f64 / client.stats.packets_sent as f64) * 100.0;

    println!("\n--- Benchmark Results ---");
    println!("Time Elapsed:    {:.2} seconds", seconds);
    println!("Total Data Sent: {:.2} MB", total_mb);
    println!("Throughput:      {:.2} MB/s", throughput_mbps);
    println!("Packet Rate:     {:.0} packets/sec", packets_per_sec);
    println!("Retransmissions: {} ({:.2}% overhead)", client.stats.retransmissions, overhead_pct);
}
