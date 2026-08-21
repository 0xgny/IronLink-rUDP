use ironlink_rudp::socket::RUdpSocket;
use std::collections::HashMap;
use std::net::SocketAddr;

fn main() {
    let mut server = RUdpSocket::bind("127.0.0.1:8080").expect("Failed to bind");
    println!("Server listening on 127.0.0.1:8080...");

    // Per-client tally, so a 5000-packet benchmark does not print 5000 lines.
    let mut seen: HashMap<SocketAddr, (usize, usize)> = HashMap::new();

    loop {
        match server.recv_reliable() {
            Ok((data, src)) => {
                let entry = seen.entry(src).or_insert((0, 0));
                entry.0 += 1;
                entry.1 += data.len();

                if data.iter().all(|b| b.is_ascii_graphic() || b.is_ascii_whitespace()) {
                    println!("{} -> {}", src, String::from_utf8_lossy(&data));
                } else if entry.0 % 1000 == 0 {
                    println!(
                        "{} -> {} packets, {:.2} MB received",
                        src,
                        entry.0,
                        entry.1 as f64 / 1_048_576.0
                    );
                }
            }
            Err(e) => eprintln!("Error receiving: {}", e),
        }
    }
}
