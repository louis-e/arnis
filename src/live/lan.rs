//! Open to LAN: a port on every network that relays to the server on loopback, so RCON stays on
//! this computer, and the announcement that lists the server under LAN worlds in Multiplayer.

use std::collections::{HashMap, HashSet};
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const DEFAULT_PORT: u16 = 25565;
/// Where the game listens for LAN worlds.
const GROUP: (Ipv4Addr, u16) = (Ipv4Addr::new(224, 0, 2, 60), 4445);
const ANNOUNCE_EVERY: Duration = Duration::from_millis(1500);
/// A client sends its handshake as soon as it connects.
const HELLO: Duration = Duration::from_secs(10);
/// The game sends keep-alives every few seconds both ways; a connection this quiet is gone.
const IDLE: Duration = Duration::from_secs(60);
/// Connections from other computers, in all and from one address.
const MAX_GUESTS: usize = 64;
const MAX_PER_GUEST: usize = 8;
/// A handshake and login start take far less.
const SNIFF_LIMIT: usize = 1024;

/// How players type an address: the port only when it is not the game's own.
pub fn address(host: &str, port: u16) -> String {
    if port == DEFAULT_PORT {
        host.to_string()
    } else {
        format!("{host}:{port}")
    }
}

pub struct Lan {
    port: u16,
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    /// The server's loopback port once it is ready, 0 before.
    server_port: AtomicU16,
    /// Lowercase names that logged in from this computer. Never removed, so nobody on the
    /// network can take them out.
    here: Mutex<HashSet<String>>,
    /// The player side of each open connection, closed when the relay ends.
    open: Mutex<HashMap<u64, TcpStream>>,
    guests: Mutex<Guests>,
}

#[derive(Default)]
struct Guests {
    total: usize,
    by_address: HashMap<IpAddr, usize>,
}

impl Guests {
    fn take(&mut self, ip: IpAddr) -> bool {
        let mine = self.by_address.entry(ip).or_default();
        if self.total >= MAX_GUESTS || *mine >= MAX_PER_GUEST {
            if *mine == 0 {
                self.by_address.remove(&ip);
            }
            return false;
        }
        *mine += 1;
        self.total += 1;
        true
    }

    fn give_back(&mut self, ip: IpAddr) {
        self.total = self.total.saturating_sub(1);
        if let Some(n) = self.by_address.get_mut(&ip) {
            *n -= 1;
            if *n == 0 {
                self.by_address.remove(&ip);
            }
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Lan {
    /// Takes the port players on the network join at: `preferred` while it is free, else any.
    /// Until `open`, connections there are closed at once, so a ping fails instead of hanging.
    pub fn bind(preferred: Option<u16>) -> Result<Self, String> {
        let on_every_network = |port: u16| TcpListener::bind((Ipv4Addr::UNSPECIFIED, port));
        let listener = preferred
            .filter(|&p| super::server::port_free(p))
            .and_then(|p| on_every_network(p).ok())
            .map_or_else(|| on_every_network(0), Ok)
            .and_then(|l| l.set_nonblocking(true).map(|()| l))
            .map_err(|e| format!("Could not open the server to your network: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let shared: Arc<Shared> = Arc::default();
        let s = shared.clone();
        let accepter = std::thread::Builder::new()
            .name("arnis-lan-accept".into())
            .spawn(move || accept(&listener, &s))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            port,
            shared,
            threads: vec![accepter],
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Relays to the server at `server_port` and lists it as `motd`. `on_address` hears the
    /// address others join at, and again whenever it changes.
    pub fn open(
        &mut self,
        server_port: u16,
        motd: &str,
        on_address: impl Fn(Option<String>) + Send + 'static,
    ) -> Result<(), String> {
        let (shared, motd, port) = (self.shared.clone(), motd.to_string(), self.port);
        let announcer = std::thread::Builder::new()
            .name("arnis-lan-announce".into())
            .spawn(move || announce(&motd, port, &shared, &on_address))
            .map_err(|e| e.to_string())?;
        self.threads.push(announcer);
        self.relay_to(server_port);
        Ok(())
    }

    fn relay_to(&self, server_port: u16) {
        self.shared
            .server_port
            .store(server_port, Ordering::Release);
    }

    /// Whether `name` has logged in from this computer this session.
    pub fn logged_in_here(&self, name: &str) -> bool {
        lock(&self.shared.here).contains(&name.to_lowercase())
    }
}

impl Drop for Lan {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        for stream in lock(&self.shared.open).values() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

fn accept(listener: &TcpListener, shared: &Arc<Shared>) {
    let mut next_id = 0u64;
    while !shared.stop.load(Ordering::Acquire) {
        let (client, peer) = match listener.accept() {
            Ok(accepted) => accepted,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        let server_port = shared.server_port.load(Ordering::Acquire);
        if server_port == 0 {
            continue;
        }
        let here = client
            .local_addr()
            .is_ok_and(|local| from_here(peer.ip(), local.ip()));
        // Only other computers are limited, so they cannot crowd out the host.
        let guest = (!here).then_some(peer.ip());
        if guest.is_some_and(|ip| !lock(&shared.guests).take(ip)) {
            continue;
        }
        next_id += 1;
        let id = next_id;
        let s = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("arnis-lan-relay".into())
            .spawn(move || {
                relay(client, server_port, here, &s, id);
                lock(&s.open).remove(&id);
                if let Some(ip) = guest {
                    lock(&s.guests).give_back(ip);
                }
            });
        if let (Err(_), Some(ip)) = (spawned, guest) {
            lock(&shared.guests).give_back(ip);
        }
    }
}

/// From this computer: over loopback, or to one of its own addresses.
fn from_here(peer: IpAddr, local: IpAddr) -> bool {
    peer.is_loopback() || peer == local
}

fn relay(client: TcpStream, server_port: u16, here: bool, shared: &Shared, id: u64) {
    // Accepted sockets keep the listener's non-blocking mode on some systems.
    let _ = client.set_nonblocking(false);
    let to = SocketAddr::from((Ipv4Addr::LOCALHOST, server_port));
    let Ok(server) = TcpStream::connect_timeout(&to, Duration::from_secs(5)) else {
        return;
    };
    let _ = client.set_read_timeout(Some(HELLO));
    let _ = server.set_read_timeout(Some(IDLE));
    for s in [&client, &server] {
        let _ = s.set_nodelay(true);
    }
    let (Ok(client_in), Ok(server_out), Ok(watch), Ok(deadline)) = (
        client.try_clone(),
        server.try_clone(),
        client.try_clone(),
        client.try_clone(),
    ) else {
        return;
    };
    lock(&shared.open).insert(id, watch);
    // Closed while this one was connecting.
    if shared.stop.load(Ordering::Acquire) {
        let _ = client.shutdown(Shutdown::Both);
        return;
    }
    let Ok(back) = std::thread::Builder::new()
        .name("arnis-lan-relay".into())
        .spawn(move || pump(server, client, &mut |_| true))
    else {
        return;
    };
    let mut seen = Vec::new();
    pump(client_in, server_out, &mut |chunk| {
        let room = SNIFF_LIMIT.saturating_sub(seen.len());
        seen.extend_from_slice(&chunk[..chunk.len().min(room)]);
        let done = match login_name(&seen) {
            Sniff::More => seen.len() >= SNIFF_LIMIT,
            Sniff::Name(name) => {
                if here {
                    lock(&shared.here).insert(name.to_lowercase());
                }
                true
            }
            Sniff::Done => true,
        };
        if done {
            let _ = deadline.set_read_timeout(Some(IDLE));
        }
        done
    });
    let _ = back.join();
}

/// Copies `from` into `to` until either side ends. `sniff` sees each chunk until it returns true.
fn pump(mut from: TcpStream, mut to: TcpStream, sniff: &mut dyn FnMut(&[u8]) -> bool) {
    let mut buf = vec![0u8; 16 * 1024];
    let mut sniffing = true;
    loop {
        match from.read(&mut buf) {
            Ok(0) => {
                // The other way may still be finishing, as after a status ping.
                let _ = to.shutdown(Shutdown::Write);
                return;
            }
            Ok(n) => {
                if sniffing && sniff(&buf[..n]) {
                    sniffing = false;
                }
                if to.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    let _ = from.shutdown(Shutdown::Both);
    let _ = to.shutdown(Shutdown::Both);
}

#[derive(Debug, PartialEq)]
enum Sniff {
    More,
    Name(String),
    Done,
}

/// A VarInt at `at`: None when the bytes run out first, Some(None) when it is too long.
fn varint(buf: &[u8], at: &mut usize) -> Option<Option<i32>> {
    let mut value = 0u32;
    for i in 0..5 {
        let b = *buf.get(*at)?;
        *at += 1;
        value |= u32::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            return Some(Some(value as i32));
        }
    }
    Some(None)
}

/// The packet at `at`, once it has arrived whole.
fn frame<'a>(buf: &'a [u8], at: &mut usize) -> Result<&'a [u8], Sniff> {
    let len = match varint(buf, at) {
        None => return Err(Sniff::More),
        Some(Some(n)) if (1..=SNIFF_LIMIT as i32).contains(&n) => n as usize,
        Some(_) => return Err(Sniff::Done),
    };
    let body = buf.get(*at..*at + len).ok_or(Sniff::More)?;
    *at += len;
    Ok(body)
}

/// A string of at most `max` characters in a whole packet.
fn string<'a>(p: &'a [u8], at: &mut usize, max: usize) -> Option<&'a str> {
    let len = usize::try_from(varint(p, at)??)
        .ok()
        .filter(|&n| n <= max * 4)?;
    let s = std::str::from_utf8(p.get(*at..*at + len)?).ok()?;
    *at += len;
    Some(s)
}

/// The name a client logs in with: the handshake asks for login, then login start names it.
/// Both come before encryption and compression.
fn login_name(buf: &[u8]) -> Sniff {
    let mut at = 0;
    let handshake = match frame(buf, &mut at) {
        Ok(p) => p,
        Err(s) => return s,
    };
    let next = (|| {
        let mut i = 0;
        (varint(handshake, &mut i)?? == 0).then_some(())?;
        varint(handshake, &mut i)??;
        string(handshake, &mut i, 255)?;
        handshake.get(i..i + 2)?;
        i += 2;
        varint(handshake, &mut i)?
    })();
    // 2 is a login and 3 a transfer from another server; 1 only asks for the status.
    if !matches!(next, Some(2 | 3)) {
        return Sniff::Done;
    }
    let start = match frame(buf, &mut at) {
        Ok(p) => p,
        Err(s) => return s,
    };
    let name = (|| {
        let mut i = 0;
        (varint(start, &mut i)?? == 0).then_some(())?;
        string(start, &mut i, 16)
    })();
    match name {
        Some(n) if !n.is_empty() => Sniff::Name(n.to_string()),
        _ => Sniff::Done,
    }
}

/// This computer's address on the network the announcement goes out on.
fn lan_ip() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(GROUP).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_unspecified() && !ip.is_loopback() => Some(ip),
        _ => None,
    }
}

/// What the game's own Open to LAN sends; the tags may not appear in the name.
fn announcement(motd: &str, port: u16) -> Vec<u8> {
    let motd: String = motd
        .chars()
        .map(|c| match c {
            '[' => '(',
            ']' => ')',
            c => c,
        })
        .collect();
    format!("[MOTD]{motd}[/MOTD][AD]{port}[/AD]").into_bytes()
}

fn announce(motd: &str, port: u16, shared: &Shared, on_address: &dyn Fn(Option<String>)) {
    let packet = announcement(motd, port);
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok();
    let mut known: Option<Option<Ipv4Addr>> = None;
    let mut failing = false;
    let mut due = Instant::now();
    while !shared.stop.load(Ordering::Acquire) {
        if Instant::now() >= due {
            due = Instant::now() + ANNOUNCE_EVERY;
            let ip = lan_ip();
            if known != Some(ip) {
                known = Some(ip);
                on_address(ip.map(|ip| address(&ip.to_string(), port)));
            }
            if let (Some(socket), Some(_)) = (&socket, ip) {
                match socket.send_to(&packet, GROUP) {
                    Ok(_) => failing = false,
                    Err(e) => {
                        if !failing {
                            eprintln!("[live] Could not list the server under LAN worlds: {e}");
                        }
                        failing = true;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_varint(out: &mut Vec<u8>, mut v: u32) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }

    fn put_string(out: &mut Vec<u8>, s: &str) {
        put_varint(out, s.len() as u32);
        out.extend_from_slice(s.as_bytes());
    }

    fn framed(body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        put_varint(&mut out, body.len() as u32);
        out.extend_from_slice(body);
        out
    }

    fn handshake(next: u32) -> Vec<u8> {
        let mut body = vec![0x00];
        put_varint(&mut body, 774);
        put_string(&mut body, "192.168.1.23");
        body.extend_from_slice(&25565u16.to_be_bytes());
        put_varint(&mut body, next);
        framed(&body)
    }

    fn login_start(name: &str) -> Vec<u8> {
        let mut body = vec![0x00];
        put_string(&mut body, name);
        body.extend_from_slice(&[7u8; 16]);
        framed(&body)
    }

    #[test]
    fn the_login_name_is_read_however_the_bytes_arrive() {
        let mut stream = handshake(2);
        stream.extend(login_start("Steve"));
        for cut in 0..stream.len() {
            assert_eq!(login_name(&stream[..cut]), Sniff::More, "cut at {cut}");
        }
        assert_eq!(login_name(&stream), Sniff::Name("Steve".into()));
        let mut transfer = handshake(3);
        transfer.extend(login_start("Alex"));
        assert_eq!(login_name(&transfer), Sniff::Name("Alex".into()));
    }

    #[test]
    fn a_padded_length_prefix_is_still_read() {
        // The server takes a length of 126 written as FE 00, so the name must be seen too.
        let mut body = vec![0x00];
        put_varint(&mut body, 774);
        put_string(&mut body, &"a".repeat(119));
        body.extend_from_slice(&25565u16.to_be_bytes());
        put_varint(&mut body, 2);
        assert_eq!(body.len(), 126);
        let mut stream = vec![0xFE, 0x00];
        stream.extend(&body);
        stream.extend(login_start("Steve"));
        assert_eq!(login_name(&stream), Sniff::Name("Steve".into()));
    }

    #[test]
    fn status_pings_and_other_bytes_name_nobody() {
        let mut status = handshake(1);
        status.extend(framed(&[0x00]));
        assert_eq!(login_name(&status), Sniff::Done);
        // The legacy ping reads as a long frame that never arrives.
        assert_eq!(login_name(&[0xFE, 0x01, 0xFA]), Sniff::More);
        assert_eq!(login_name(&[0xFF, 0xFF, 0xFF, 0xFF, 0x7F]), Sniff::Done);
        assert_eq!(login_name(&b"GET / HTTP/1.1\r\n".repeat(6)), Sniff::Done);
        let mut long_name = handshake(2);
        long_name.extend(login_start(&"x".repeat(80)));
        assert_eq!(login_name(&long_name), Sniff::Done);
    }

    #[test]
    fn the_announcement_is_the_games_own() {
        assert_eq!(
            announcement("Arnis Live \u{00b7} World", 25565),
            "[MOTD]Arnis Live \u{00b7} World[/MOTD][AD]25565[/AD]".as_bytes()
        );
        assert_eq!(
            announcement("A [/MOTD] B", 40000),
            b"[MOTD]A (/MOTD) B[/MOTD][AD]40000[/AD]"
        );
    }

    #[test]
    fn addresses_name_the_port_only_when_it_is_not_the_default() {
        assert_eq!(address("localhost", 25565), "localhost");
        assert_eq!(address("192.168.1.23", 25566), "192.168.1.23:25566");
    }

    #[test]
    fn this_computer_is_told_from_the_network() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert!(from_here(ip("127.0.0.1"), ip("127.0.0.1")));
        assert!(from_here(ip("192.168.1.23"), ip("192.168.1.23")));
        assert!(!from_here(ip("192.168.1.40"), ip("192.168.1.23")));
    }

    #[test]
    fn the_relay_carries_both_ways_and_records_who_logged_in() {
        let server = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let server_port = server.local_addr().unwrap().port();
        let echo = std::thread::spawn(move || {
            let (mut s, _) = server.accept().unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 256];
            loop {
                let n = s.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
                s.write_all(&buf[..n]).unwrap();
            }
            got
        });
        let lan = Lan::bind(None).unwrap();
        lan.relay_to(server_port);
        let mut hello = handshake(2);
        hello.extend(login_start("Steve"));
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, lan.port())).unwrap();
        client.write_all(&hello).unwrap();
        let mut back = vec![0u8; hello.len()];
        client.read_exact(&mut back).unwrap();
        assert_eq!(back, hello);
        client.shutdown(Shutdown::Write).unwrap();
        assert_eq!(client.read(&mut [0u8; 8]).unwrap(), 0);
        assert_eq!(echo.join().unwrap(), hello);
        assert!(lan.logged_in_here("steve"));
        assert!(!lan.logged_in_here("nobody"));
    }

    #[test]
    fn before_the_server_is_ready_connections_are_closed_at_once() {
        let lan = Lan::bind(None).unwrap();
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, lan.port())).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let start = Instant::now();
        let closed = matches!(client.read(&mut [0u8; 8]), Ok(0) | Err(_));
        assert!(closed && start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn other_computers_get_a_share_of_the_connections() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let mut guests = Guests::default();
        for _ in 0..MAX_PER_GUEST {
            assert!(guests.take(ip("192.168.1.40")));
        }
        assert!(
            !guests.take(ip("192.168.1.40")),
            "one address has its share"
        );
        assert!(guests.take(ip("192.168.1.41")));
        guests.give_back(ip("192.168.1.40"));
        assert!(guests.take(ip("192.168.1.40")));
        let mut full = Guests::default();
        for n in 0..MAX_GUESTS {
            assert!(full.take(IpAddr::from([10, 0, (n / 200) as u8, (n % 200) as u8])));
        }
        assert!(!full.take(ip("10.9.9.9")), "all of them have theirs");
        assert!(!full.by_address.contains_key(&ip("10.9.9.9")));
    }

    #[test]
    fn closing_ends_the_connections_it_relays() {
        let server = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let server_port = server.local_addr().unwrap().port();
        let held = std::thread::spawn(move || server.accept().map(|(s, _)| s));
        let lan = Lan::bind(None).unwrap();
        lan.relay_to(server_port);
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, lan.port())).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let _server_side = held.join().unwrap().unwrap();
        let open = Instant::now();
        while lock(&lan.shared.open).is_empty() {
            assert!(open.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(lan);
        let ended = matches!(client.read(&mut [0u8; 8]), Ok(0) | Err(_));
        assert!(ended, "the connection outlived the relay");
    }
}
