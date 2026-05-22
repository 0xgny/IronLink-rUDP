use ironlink_rudp::socket::RUdpSocket;
use std::thread;
use std::time::Duration;

fn main() {
    let mut client = RUdpSocket::bind("127.0.0.1:0").expect("Failed to bind");
    let server_addr = "127.0.0.1:8080";

    let messages = ["Hello", "Reliable", "UDP", "World!"];

    for msg in messages.iter() {
        client.send_reliable(msg.as_bytes(), server_addr).expect("Send failed");
        thread::sleep(Duration::from_millis(100)); // Slight delay to observe flow
    }
}
