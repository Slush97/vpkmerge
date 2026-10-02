//! Drives the real binary over stdio as an MCP host would: handshake, tool list,
//! and tool calls that need no game install.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

struct Host {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Host {
    /// Start the server with no discoverable Deadlock install.
    fn start(home: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_vpkmerge-mcp"))
            .env_remove("DEADLOCK_PAK")
            .env_remove("DEADLOCK_GAME_DIR")
            .env_remove("VPKMERGE_DOCS_DIRS")
            .env_remove("XDG_DATA_HOME")
            .env("HOME", home)
            .env("VPKMERGE_STAGING_DIR", home.join("staging"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn vpkmerge-mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut host = Self {
            child,
            stdin,
            stdout,
            next_id: 0,
        };
        let init = host.request(
            "initialize",
            &json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "0" }
            }),
        );
        assert!(init["instructions"]
            .as_str()
            .unwrap()
            .contains("Discover first"));
        host.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        host
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.stdin, "{message}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: &Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let mut line = String::new();
            assert!(
                self.stdout.read_line(&mut line).unwrap() > 0,
                "server closed stdout"
            );
            let message: Value = serde_json::from_str(&line).expect("server wrote non-JSON");
            if message["id"] == id {
                assert!(message["error"].is_null(), "{method} failed: {message}");
                return message["result"].clone();
            }
        }
    }

    fn call(&mut self, tool: &str, arguments: &Value) -> Value {
        self.request(
            "tools/call",
            &json!({ "name": tool, "arguments": arguments }),
        )
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn serves_tools_docs_and_recoverable_errors() {
    let home = tempfile::tempdir().unwrap();
    let mut host = Host::start(home.path());

    let tools = host.request("tools/list", &json!({}));
    let mut names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "browse_sounds",
            "browse_textures",
            "game_status",
            "inspect_mod",
            "list_hero_animations",
            "list_hero_textures",
            "list_heroes",
            "make_sound_louder",
            "map_texture_regions",
            "merge_mods",
            "open_in_grimoire",
            "preview_hero",
            "preview_hero_animation",
            "read_doc",
            "recolor_hero_vfx",
            "replace_textures",
            "search_docs",
            "swap_hero_sound",
            "swap_sound_clip",
        ]
    );
    for tool in tools["tools"].as_array().unwrap() {
        assert_eq!(tool["inputSchema"]["type"], "object", "{}", tool["name"]);
        assert!(!tool["description"].as_str().unwrap().is_empty());
        // Hosts ask the user before tools that do not rule these out. Every tool
        // here is local, and the ones that write only add files.
        let hints = &tool["annotations"];
        assert_eq!(hints["openWorldHint"], false, "{}", tool["name"]);
        assert!(
            hints["readOnlyHint"] == true || hints["destructiveHint"] == false,
            "{}",
            tool["name"]
        );
    }

    // The knowledge base needs no game install.
    let found = host.call(
        "search_docs",
        &json!({ "query": "randomizer pool sound event" }),
    );
    let hit = &found["structuredContent"]["results"][0];
    assert_eq!(hit["doc"], "guides/audio-soundevents.md");
    let section = host.call(
        "read_doc",
        &json!({ "doc": hit["doc"], "section": hit["section"] }),
    );
    assert!(section["content"][0]["text"]
        .as_str()
        .unwrap()
        .starts_with('#'));

    // A tool that needs the game fails as a tool error that says how to fix it.
    let heroes = host.call("list_heroes", &json!({}));
    assert_eq!(heroes["isError"], true);
    assert!(heroes["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("DEADLOCK_GAME_DIR"));

    let status = host.call("game_status", &json!({}));
    let status = &status["structuredContent"];
    assert!(status["pak"].is_null());
    assert!(status["problem"]
        .as_str()
        .unwrap()
        .contains("DEADLOCK_GAME_DIR"));
    assert!(status["stagingDir"].as_str().unwrap().ends_with("staging"));
}
