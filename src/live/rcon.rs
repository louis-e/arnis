//! Minimal RCON client for the local live server, and parsers for its replies.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

const AUTH: i32 = 3;
const EXEC: i32 = 2;
/// Vanilla splits longer replies into packets of this many bytes.
const FRAGMENT: usize = 4096;

pub struct Rcon {
    stream: TcpStream,
    next_id: i32,
}

impl Rcon {
    pub fn connect(port: u16, password: &str) -> Result<Self, String> {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
            .map_err(|e| format!("RCON connect: {e}"))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .map_err(|e| e.to_string())?;
        let _ = stream.set_nodelay(true);
        let mut rcon = Rcon { stream, next_id: 1 };
        let id = rcon.send(AUTH, password)?;
        loop {
            let (rid, kind, _) = rcon.recv()?;
            if rid == -1 {
                return Err("the server refused the RCON password".to_string());
            }
            // Vanilla sends the auth reply as type 2.
            if rid == id && kind == EXEC {
                return Ok(rcon);
            }
        }
    }

    fn send(&mut self, kind: i32, body: &str) -> Result<i32, String> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let mut packet = Vec::with_capacity(body.len() + 14);
        packet.extend_from_slice(&((body.len() + 10) as i32).to_le_bytes());
        packet.extend_from_slice(&id.to_le_bytes());
        packet.extend_from_slice(&kind.to_le_bytes());
        packet.extend_from_slice(body.as_bytes());
        packet.extend_from_slice(&[0, 0]);
        self.stream
            .write_all(&packet)
            .map_err(|e| format!("RCON send: {e}"))?;
        Ok(id)
    }

    fn recv(&mut self) -> Result<(i32, i32, String), String> {
        let mut len = [0u8; 4];
        self.stream
            .read_exact(&mut len)
            .map_err(|e| format!("RCON read: {e}"))?;
        let len = i32::from_le_bytes(len);
        if !(10..=1 << 20).contains(&len) {
            return Err(format!("RCON packet of {len} bytes"));
        }
        let mut buf = vec![0u8; len as usize];
        self.stream
            .read_exact(&mut buf)
            .map_err(|e| format!("RCON read: {e}"))?;
        let id = i32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        let kind = i32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let body = String::from_utf8_lossy(&buf[8..buf.len() - 2]).into_owned();
        Ok((id, kind, body))
    }

    /// Runs one command and returns everything it printed. Vanilla parses one packet per
    /// read and drops the connection on more, so nothing else may be sent until it answers.
    pub fn cmd(&mut self, command: &str) -> Result<String, String> {
        let id = self.send(EXEC, command)?;
        let mut out = String::new();
        loop {
            let (rid, _, body) = self.recv()?;
            if rid != id {
                continue;
            }
            let full = body.len() >= FRAGMENT;
            out.push_str(&body);
            if !full {
                return Ok(out);
            }
            // A full packet may have more behind it; a short wait tells.
            let _ = self
                .stream
                .set_read_timeout(Some(Duration::from_millis(50)));
            let more = self.recv();
            let _ = self.stream.set_read_timeout(Some(Duration::from_secs(30)));
            match more {
                Ok((rid, _, body)) if rid == id => out.push_str(&body),
                _ => return Ok(out),
            }
        }
    }
}

/// A position in an RCON `data get entity` reply.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerPos {
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

const ENTITY_DATA: &str = " has the following entity data: ";

/// Parses the concatenated replies of `execute as @a ... run data get entity @s Pos`.
pub fn parse_positions(reply: &str) -> Vec<PlayerPos> {
    let mut out = Vec::new();
    let mut rest = reply;
    while let Some(at) = rest.find(ENTITY_DATA) {
        let name_start = rest[..at]
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .map_or(0, |i| i + 1);
        let name = rest[name_start..at].to_string();
        let after = &rest[at + ENTITY_DATA.len()..];
        let Some(open) = after.find('[') else { break };
        let Some(close) = after.find(']') else { break };
        let nums: Vec<f64> = after[open + 1..close]
            .split(',')
            .filter_map(|v| v.trim().trim_end_matches(['d', 'D']).parse().ok())
            .collect();
        if nums.len() == 3 && !name.is_empty() && nums.iter().all(|v| v.is_finite()) {
            out.push(PlayerPos {
                name,
                x: nums[0],
                y: nums[1],
                z: nums[2],
            });
        }
        rest = &after[close + 1..];
    }
    out
}

/// Names in a `list` reply: "There are 1 of a max of 8 players online: Alex, Steve".
pub fn parse_list(reply: &str) -> Vec<String> {
    let Some((_, names)) = reply.split_once(": ") else {
        return Vec::new();
    };
    names
        .split(',')
        .map(|n| n.trim())
        .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .map(str::to_string)
        .collect()
}

/// A JSON text component, valid as both 1.21.4 JSON and 1.21.5+ SNBT.
pub fn text(message: &str, color: &str) -> String {
    serde_json::json!({ "text": message, "color": color }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_of_several_players_are_split() {
        let reply = "Alex has the following entity data: [12.5d, -1520.0d, -3.25d]\
                     Steve_2 has the following entity data: [1.0d, 2.0d, 3.0d]";
        let got = parse_positions(reply);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "Alex");
        assert_eq!((got[0].x, got[0].y, got[0].z), (12.5, -1520.0, -3.25));
        assert_eq!(got[1].name, "Steve_2");
    }

    #[test]
    fn an_empty_or_odd_reply_has_no_positions() {
        assert!(parse_positions("").is_empty());
        assert!(parse_positions("No entity was found").is_empty());
    }

    #[test]
    fn list_names_are_read() {
        assert_eq!(
            parse_list("There are 2 of a max of 8 players online: Alex, Steve"),
            vec!["Alex", "Steve"]
        );
        assert!(parse_list("There are 0 of a max of 8 players online: ").is_empty());
    }
}
