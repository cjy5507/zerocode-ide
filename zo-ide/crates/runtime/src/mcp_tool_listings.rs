//! A local MCP server's tool list, remembered between sessions.
//!
//! Learning what a server offers means starting it, and a session started
//! every configured server for that alone: ten processes and about 1.45 GB on
//! one machine (2026-11-11), most of them stopped a minute and a half later
//! without a call ([`crate::McpServerManager::release_unused_servers`]). What a
//! server answered is kept here instead, filed under how the server is started
//! ([`crate::McpServerManager::tool_listing_key`]). A session that finds a
//! listing that still stands leaves the server alone until something calls
//! it, and that first call asks the server itself before it is sent
//! ([`crate::McpServerManager::call_tool`]).
//!
//! A listing is what a process this machine started said about itself, read
//! back by the same user's sessions: it carries no more trust than the
//! server's own answer did, and no less.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::mcp_stdio::McpTool;

/// How long a listing stands in for asking the server. A server that a
/// session calls is asked at that call whatever the listing's age, so this
/// bounds one thing only: how long a tool gained by a server nobody calls
/// stays unseen.
pub const LISTING_STANDS_FOR: Duration = Duration::from_secs(24 * 60 * 60);

/// The folder under zo's cache that holds the listings.
const LISTINGS_DIR: &str = "mcp-tools";
/// The shape of a listing file. A file of another shape is no listing.
const LISTING_VERSION: u32 = 1;

/// Where listings are kept, and for how long one is taken at its word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolListings {
    pub dir: PathBuf,
    pub stands_for: Duration,
}

/// One listing file.
#[derive(Debug, Serialize, Deserialize)]
struct Listing {
    version: u32,
    /// The server's configured name, for a person reading the folder.
    server: String,
    listed_unix: u64,
    tools: Vec<McpTool>,
}

impl ToolListings {
    /// `<zo home>/cache/mcp-tools`, each listing good for
    /// [`LISTING_STANDS_FOR`].
    #[must_use]
    pub fn in_zo_home() -> Self {
        Self {
            dir: crate::model_discovery::cache_home().join("cache").join(LISTINGS_DIR),
            stands_for: LISTING_STANDS_FOR,
        }
    }

    fn file(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }

    /// The tools listed under `key`, when that listing still stands at `now`.
    /// A missing file, one that does not read as a listing, one too old and
    /// one dated after `now` — a clock set back — are all no listing.
    #[must_use]
    pub fn recall(&self, key: &str, now: SystemTime) -> Option<Vec<McpTool>> {
        let text = fs::read_to_string(self.file(key)).ok()?;
        let listing: Listing = serde_json::from_str(&text).ok()?;
        if listing.version != LISTING_VERSION {
            return None;
        }
        let listed = UNIX_EPOCH + Duration::from_secs(listing.listed_unix);
        let age = now.duration_since(listed).ok()?;
        (age < self.stands_for).then_some(listing.tools)
    }

    /// Keep `tools` as what `server` listed at `now`.
    ///
    /// The file is written beside its place and renamed over it, so a session
    /// reading meanwhile finds the old listing or the new one and never a
    /// part of either. The folder and the file are the person's alone, as
    /// zo's other state is: a listing is what a session shows the model, and
    /// what each tool's permission is read from until its first call.
    pub fn remember(
        &self,
        key: &str,
        server: &str,
        tools: &[McpTool],
        now: SystemTime,
    ) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        core_types::paths::restrict_permissions_owner_only(&self.dir)?;
        let listing = Listing {
            version: LISTING_VERSION,
            server: server.to_string(),
            listed_unix: now.duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs()),
            tools: tools.to_vec(),
        };
        let text = serde_json::to_string(&listing).map_err(io::Error::other)?;
        let partial = self.dir.join(format!("{key}.{}.partial", std::process::id()));
        core_types::paths::write_private_file(
            &partial,
            text.as_bytes(),
            &core_types::paths::ParentDirPolicy::LeaveParent,
        )?;
        fs::rename(&partial, self.file(key)).inspect_err(|_| {
            let _ = fs::remove_file(&partial);
        })
    }

    /// Stop standing on the listing under `key`: the next session asks the
    /// server. For a server that could not be started when its first call
    /// came — a session opening on its old listing would offer tools that
    /// cannot be called, and say nothing of the failure.
    pub fn forget(&self, key: &str) {
        let _ = fs::remove_file(self.file(key));
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use serde_json::json;

    use super::{ToolListings, LISTING_STANDS_FOR};
    use crate::mcp_stdio::McpTool;

    fn listings(name: &str) -> ToolListings {
        let dir = std::env::temp_dir().join(format!("zo-mcp-listings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        ToolListings { dir, stands_for: LISTING_STANDS_FOR }
    }

    fn tool(name: &str) -> McpTool {
        McpTool {
            name: name.to_string(),
            description: Some(format!("{name} of a test server")),
            input_schema: Some(json!({ "type": "object", "properties": { "text": { "type": "string" } } })),
            annotations: Some(json!({ "readOnlyHint": true })),
            meta: None,
        }
    }

    /// What was remembered comes back whole — names, descriptions, schemas,
    /// annotations — for as long as the listing stands, and not after.
    #[test]
    fn a_listing_comes_back_whole_until_it_no_longer_stands() {
        let listings = listings("whole");
        let now = SystemTime::now();
        let tools = vec![tool("echo"), tool("search")];
        assert_eq!(listings.recall("alpha-1", now), None, "nothing was remembered yet");
        listings.remember("alpha-1", "alpha", &tools, now).expect("remember");

        assert_eq!(listings.recall("alpha-1", now), Some(tools.clone()));
        assert_eq!(
            listings.recall("alpha-1", now + LISTING_STANDS_FOR - Duration::from_secs(1)),
            Some(tools),
            "a second before it stops standing"
        );
        assert_eq!(listings.recall("alpha-1", now + LISTING_STANDS_FOR), None, "too old");
        assert_eq!(listings.recall("alpha-1", now - Duration::from_secs(60)), None, "dated after now");
        assert_eq!(listings.recall("alpha-2", now), None, "another server's key");

        listings.forget("alpha-1");
        assert_eq!(listings.recall("alpha-1", now), None, "a forgotten listing is no listing");
        listings.forget("alpha-1");
        let _ = std::fs::remove_dir_all(&listings.dir);
    }

    /// A later listing replaces the earlier one, and nothing but listings is
    /// left in the folder.
    #[test]
    fn a_later_listing_replaces_the_earlier_one_and_leaves_no_partial_file() {
        let listings = listings("replace");
        let now = SystemTime::now();
        listings.remember("alpha-1", "alpha", &[tool("echo")], now).expect("first");
        listings.remember("alpha-1", "alpha", &[tool("echo"), tool("more")], now).expect("second");

        let names: Vec<String> = listings
            .recall("alpha-1", now)
            .expect("the listing")
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        assert_eq!(names, ["echo", "more"]);
        let files: Vec<String> = std::fs::read_dir(&listings.dir)
            .expect("the folder")
            .map(|entry| entry.expect("an entry").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(files, ["alpha-1.json"]);
        let _ = std::fs::remove_dir_all(&listings.dir);
    }

    /// The folder and a listing in it are the person's alone.
    #[cfg(unix)]
    #[test]
    fn a_listing_and_its_folder_are_the_owners_alone() {
        use std::os::unix::fs::PermissionsExt as _;

        let listings = listings("private");
        listings.remember("alpha-1", "alpha", &[tool("echo")], SystemTime::now()).expect("remember");
        let mode = |path: &std::path::Path| std::fs::metadata(path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode(&listings.dir), 0o700, "the folder");
        assert_eq!(mode(&listings.dir.join("alpha-1.json")), 0o600, "the listing");
        let _ = std::fs::remove_dir_all(&listings.dir);
    }

    /// A file that is not a listing — cut short, another shape, another
    /// version — is no listing, and the next one written reads again.
    #[test]
    fn a_file_that_is_not_a_listing_is_no_listing() {
        let listings = listings("broken");
        let now = SystemTime::now();
        std::fs::create_dir_all(&listings.dir).expect("the folder");
        for broken in ["", "{\"version\":1,\"server\":\"alpha\"", "[1,2,3]", "{\"version\":2,\"server\":\"alpha\",\"listed_unix\":0,\"tools\":[]}"] {
            std::fs::write(listings.dir.join("alpha-1.json"), broken).expect("write");
            assert_eq!(listings.recall("alpha-1", now), None, "{broken:?}");
        }
        listings.remember("alpha-1", "alpha", &[tool("echo")], now).expect("remember");
        assert!(listings.recall("alpha-1", now).is_some_and(|tools| tools.len() == 1));
        let _ = std::fs::remove_dir_all(&listings.dir);
    }
}
