use obscura_net::is_forbidden_ip;
use std::net::IpAddr;

fn ip(value: &str) -> IpAddr { value.parse().unwrap() }

#[test]
fn teredo_cannot_hide_a_forbidden_ipv4() {
    // RFC 4380: 2001:0::/32 embeds the Teredo server's IPv4 in bits
    // 32..64 and the client's public IPv4, bit-inverted, in the last 32
    // bits. Both are IPv4-in-IPv6 forms like NAT64 and 6to4.
    // Client 127.1.1.1 (inverted 80fe:fefe) behind server 65.54.227.120.
    assert!(is_forbidden_ip(ip("2001:0:4136:e378:8000:63bf:80fe:fefe")));
    // Client 10.0.0.1 (inverted f5ff:fffe).
    assert!(is_forbidden_ip(ip("2001:0:4136:e378:8000:63bf:f5ff:fffe")));
    // Server 10.0.0.1 with a public client.
    assert!(is_forbidden_ip(ip("2001:0:a00:1:8000:63bf:f7f7:f7f7")));
    // Public server and public client 8.8.8.8 (inverted f7f7:f7f7).
    assert!(!is_forbidden_ip(ip("2001:0:4136:e378:8000:63bf:f7f7:f7f7")));
}
