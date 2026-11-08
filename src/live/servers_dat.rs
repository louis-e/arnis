//! The "Arnis Live" entry in the game's multiplayer list (`servers.dat`).

use fastnbt::Value;
use std::collections::HashMap;
use std::path::Path;

pub const ENTRY_NAME: &str = "Arnis Live";

/// Puts the entry first in the list, or points the existing one at `address`.
pub fn ensure_entry(game_dir: &Path, address: &str) -> Result<(), String> {
    let path = game_dir.join("servers.dat");
    let mut root = match std::fs::read(&path) {
        Ok(bytes) => fastnbt::from_bytes::<Value>(&bytes)
            .map_err(|e| format!("servers.dat is unreadable: {e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Compound(HashMap::new()),
        Err(e) => return Err(e.to_string()),
    };
    let Value::Compound(top) = &mut root else {
        return Err("servers.dat has an unexpected layout".to_string());
    };
    let servers = top
        .entry("servers".to_string())
        .or_insert_with(|| Value::List(Vec::new()));
    let Value::List(list) = servers else {
        return Err("servers.dat has an unexpected layout".to_string());
    };
    let existing = list.iter_mut().find_map(|s| match s {
        Value::Compound(c) if matches!(c.get("name"), Some(Value::String(n)) if n == ENTRY_NAME) => Some(c),
        _ => None,
    });
    match existing {
        Some(entry) => {
            if matches!(entry.get("ip"), Some(Value::String(ip)) if ip == address) {
                return Ok(());
            }
            entry.insert("ip".to_string(), Value::String(address.to_string()));
        }
        None => {
            let mut entry = HashMap::new();
            entry.insert("name".to_string(), Value::String(ENTRY_NAME.to_string()));
            entry.insert("ip".to_string(), Value::String(address.to_string()));
            entry.insert("hidden".to_string(), Value::Byte(0));
            list.insert(0, Value::Compound(entry));
        }
    }
    let bytes = fastnbt::to_bytes(&root).map_err(|e| e.to_string())?;
    crate::world_utils::replace_file_atomically(&path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names_and_ips(dir: &Path) -> Vec<(String, String)> {
        let v: Value =
            fastnbt::from_bytes(&std::fs::read(dir.join("servers.dat")).unwrap()).unwrap();
        let Value::Compound(top) = v else { panic!() };
        let Some(Value::List(list)) = top.get("servers") else {
            panic!()
        };
        list.iter()
            .map(|s| {
                let Value::Compound(c) = s else { panic!() };
                let get = |k: &str| match c.get(k) {
                    Some(Value::String(s)) => s.clone(),
                    _ => String::new(),
                };
                (get("name"), get("ip"))
            })
            .collect()
    }

    #[test]
    fn the_entry_is_added_once_and_kept_up_to_date() {
        let dir = tempfile::tempdir().unwrap();
        ensure_entry(dir.path(), "localhost").unwrap();
        ensure_entry(dir.path(), "localhost:25570").unwrap();
        assert_eq!(
            names_and_ips(dir.path()),
            vec![(ENTRY_NAME.to_string(), "localhost:25570".to_string())]
        );
    }

    #[test]
    fn other_servers_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let mut friend = HashMap::new();
        friend.insert("name".to_string(), Value::String("Friends".to_string()));
        friend.insert(
            "ip".to_string(),
            Value::String("mc.example.org".to_string()),
        );
        let mut top = HashMap::new();
        top.insert(
            "servers".to_string(),
            Value::List(vec![Value::Compound(friend)]),
        );
        std::fs::write(
            dir.path().join("servers.dat"),
            fastnbt::to_bytes(&Value::Compound(top)).unwrap(),
        )
        .unwrap();
        ensure_entry(dir.path(), "localhost").unwrap();
        let got = names_and_ips(dir.path());
        assert_eq!(got[0].0, ENTRY_NAME);
        assert_eq!(
            got[1],
            ("Friends".to_string(), "mc.example.org".to_string())
        );
    }
}
