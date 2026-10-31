//! L-701 (TST-SUP-001): the planted egress probe for `scripts/sup/egress.sh`.
//!
//! Without `EDRS_EGRESS_PLANT` this does nothing, so it is harmless in every other run (it is
//! also outside the nextest default filters). With `EDRS_EGRESS_PLANT=dns` it resolves
//! `example.com`; with `EDRS_EGRESS_PLANT=tcp` it opens a TCP connection to a non-loopback
//! address (192.0.2.1, TEST-NET-1). The test itself passes either way: what must fail is the
//! egress-denied job, which counts the DNS query (logging sink) or the unroutable connection
//! attempt (OutNoRoutes in the run's network namespace). `egress.sh --selftest` proves both.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

#[test]
fn egress_plant() {
    match std::env::var("EDRS_EGRESS_PLANT").as_deref() {
        Ok("dns") => {
            let resolved = "example.com:80".to_socket_addrs().map(|a| a.count());
            println!("EGRESS-PLANT dns example.com -> {resolved:?}");
        }
        Ok("tcp") => {
            let addr: SocketAddr = "192.0.2.1:80".parse().expect("literal address");
            let connected = TcpStream::connect_timeout(&addr, Duration::from_secs(1)).map(|_| ());
            println!("EGRESS-PLANT tcp {addr} -> {connected:?}");
        }
        _ => {}
    }
}
