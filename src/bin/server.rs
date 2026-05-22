use ironlink_rudp::socket::RUdpSocket;

fn main() {
    let mut server = RUdpSocket::bind("127.0.0.1:8080").expect("Failed to bind");
    println!("Server listening on 127.0.0.1:8080...");

    loop {
        match server.recv_reliable() {
            Ok((data, src)) => {
                let msg = String::from_utf8_lossy(&data);
                println!("Received from {}: {}", src, msg);
            }
            Err(e) => eprintln!("Error receiving: {}", e),
        }
    }
}
