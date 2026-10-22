//! Text KeyValues (VDF) parsing of a PICS app-info buffer and selection of
//! the branch build id and depot manifests.
//!
//! Keys compare ASCII case-insensitively, as KeyValues does. Conditional
//! `[$...]` suffixes and `//` comments are skipped. Steam's documented/observed
//! format (SteamKit2 as reference, independently implemented).

use anyhow::{Context, Result, bail, ensure};

/// Bounds on one parsed document.
const MAX_DEPTH: usize = 64;
const MAX_NODES: usize = 1_000_000;

/// A KeyValues value: a string or an ordered list of children.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Obj(Vec<(String, Value)>),
}

impl Value {
    /// First child named `key` (ASCII case-insensitive).
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(children) => children
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(key))
                .map(|(_, value)| value),
            Value::Str(_) => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(text) => Some(text),
            Value::Obj(_) => None,
        }
    }

    pub fn children(&self) -> &[(String, Value)] {
        match self {
            Value::Obj(children) => children,
            Value::Str(_) => &[],
        }
    }

    /// Follow a path of keys.
    pub fn path(&self, keys: &[&str]) -> Option<&Value> {
        keys.iter().try_fold(self, |value, key| value.get(key))
    }
}

#[derive(Debug, PartialEq)]
enum Token {
    Str(String),
    Open,
    Close,
}

struct Lexer<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Lexer<'_> {
    fn next(&mut self) -> Result<Option<Token>> {
        loop {
            while self.pos < self.bytes.len()
                && (self.bytes[self.pos].is_ascii_whitespace() || self.bytes[self.pos] == 0)
            {
                self.pos += 1;
            }
            let Some(&byte) = self.bytes.get(self.pos) else {
                return Ok(None);
            };
            match byte {
                b'/' if self.bytes.get(self.pos + 1) == Some(&b'/') => {
                    while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                }
                b'[' => {
                    // Conditional such as [$WIN32]; ignored.
                    while self.pos < self.bytes.len() && self.bytes[self.pos] != b']' {
                        self.pos += 1;
                    }
                    ensure!(self.pos < self.bytes.len(), "unterminated VDF conditional");
                    self.pos += 1;
                }
                b'{' => {
                    self.pos += 1;
                    return Ok(Some(Token::Open));
                }
                b'}' => {
                    self.pos += 1;
                    return Ok(Some(Token::Close));
                }
                b'"' => return self.quoted().map(Some),
                _ => return self.bare().map(Some),
            }
        }
    }

    fn quoted(&mut self) -> Result<Token> {
        self.pos += 1;
        let mut out = Vec::new();
        loop {
            let Some(&byte) = self.bytes.get(self.pos) else {
                bail!("unterminated VDF string");
            };
            self.pos += 1;
            match byte {
                b'"' => break,
                b'\\' => {
                    let Some(&escaped) = self.bytes.get(self.pos) else {
                        bail!("unterminated VDF escape");
                    };
                    self.pos += 1;
                    out.push(match escaped {
                        b'n' => b'\n',
                        b't' => b'\t',
                        b'r' => b'\r',
                        other => other,
                    });
                }
                other => out.push(other),
            }
        }
        Ok(Token::Str(
            String::from_utf8(out).context("VDF string is not UTF-8")?,
        ))
    }

    fn bare(&mut self) -> Result<Token> {
        let start = self.pos;
        while self.pos < self.bytes.len() {
            let byte = self.bytes[self.pos];
            if byte.is_ascii_whitespace() || matches!(byte, b'{' | b'}' | b'"' | 0) {
                break;
            }
            self.pos += 1;
        }
        Ok(Token::Str(
            std::str::from_utf8(&self.bytes[start..self.pos])
                .context("VDF token is not UTF-8")?
                .to_owned(),
        ))
    }
}

/// Parse a text KeyValues document into its root `Obj`. Usually the root
/// holds one key (`appinfo`) whose object is the app.
pub fn parse(bytes: &[u8]) -> Result<Value> {
    let mut lexer = Lexer { bytes, pos: 0 };
    let mut nodes = 0usize;
    let root = parse_object(&mut lexer, 0, &mut nodes, true)?;
    Ok(Value::Obj(root))
}

fn parse_object(
    lexer: &mut Lexer<'_>,
    depth: usize,
    nodes: &mut usize,
    top: bool,
) -> Result<Vec<(String, Value)>> {
    ensure!(depth <= MAX_DEPTH, "VDF nested deeper than {MAX_DEPTH}");
    let mut children = Vec::new();
    loop {
        let key = match lexer.next()? {
            None if top => return Ok(children),
            None => bail!("VDF object is not closed"),
            Some(Token::Close) if !top => return Ok(children),
            Some(Token::Close) => bail!("unbalanced VDF close brace"),
            Some(Token::Open) => bail!("VDF object without a key"),
            Some(Token::Str(key)) => key,
        };
        *nodes += 1;
        ensure!(*nodes <= MAX_NODES, "VDF holds more than {MAX_NODES} nodes");
        let value = match lexer.next()? {
            Some(Token::Str(text)) => Value::Str(text),
            Some(Token::Open) => Value::Obj(parse_object(lexer, depth + 1, nodes, false)?),
            Some(Token::Close) | None => bail!("VDF key {key:?} has no value"),
        };
        children.push((key, value));
    }
}

/// A depot's public manifest as listed in app info.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepotEntry {
    pub depot_id: u32,
    pub manifest_gid: u64,
    /// `config/oslist`, empty when unset.
    pub oslist: String,
    /// Size from the manifest entry, when listed.
    pub size: Option<u64>,
}

/// Branch build id and depot manifests selected from an app-info document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDepots {
    pub build_id: Option<u64>,
    pub depots: Vec<DepotEntry>,
}

/// Find the `appinfo` object, accepting a document rooted at it or not.
fn app_root(doc: &Value) -> &Value {
    doc.get("appinfo").unwrap_or(doc)
}

/// Select every depot with a manifest for `branch`, plus the branch build
/// id. Depots without a manifest (shared or `depotfromapp`) are skipped.
pub fn app_depots(doc: &Value, branch: &str) -> Result<AppDepots> {
    let app = app_root(doc);
    let depots = app
        .get("depots")
        .context("app info has no depots section")?;
    let build_id = depots
        .path(&["branches", branch, "buildid"])
        .and_then(Value::as_str)
        .map(|text| {
            text.parse::<u64>()
                .context("branch buildid is not a number")
        })
        .transpose()?;
    let mut out = Vec::new();
    for (key, depot) in depots.children() {
        let Ok(depot_id) = key.parse::<u32>() else {
            continue;
        };
        let Some(manifest) = depot.path(&["manifests", branch]) else {
            continue;
        };
        // Current layout: manifests/<branch>/{gid,size,download}; older
        // layout: manifests/<branch> is the gid string itself.
        let (gid, size) = match manifest {
            Value::Str(gid) => (gid.as_str(), None),
            Value::Obj(_) => (
                manifest
                    .get("gid")
                    .and_then(Value::as_str)
                    .context("depot manifest has no gid")?,
                manifest
                    .get("size")
                    .and_then(Value::as_str)
                    .and_then(|text| text.parse::<u64>().ok()),
            ),
        };
        let manifest_gid = gid
            .parse::<u64>()
            .with_context(|| format!("depot {depot_id} manifest gid is not a number"))?;
        let oslist = depot
            .path(&["config", "oslist"])
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        out.push(DepotEntry {
            depot_id,
            manifest_gid,
            oslist,
            size,
        });
    }
    Ok(AppDepots {
        build_id,
        depots: out,
    })
}

/// Candidate order: depots for Windows or for every OS first, then the rest,
/// each group in app-info order.
pub fn windows_first(depots: &[DepotEntry]) -> Vec<DepotEntry> {
    let windows = |entry: &DepotEntry| {
        entry.oslist.is_empty()
            || entry
                .oslist
                .split(',')
                .any(|os| os.trim().eq_ignore_ascii_case("windows"))
    };
    let mut out: Vec<DepotEntry> = depots.iter().filter(|d| windows(d)).cloned().collect();
    out.extend(depots.iter().filter(|d| !windows(d)).cloned());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_case_insensitive_and_bare_tokens_parse() {
        let doc = parse(b"Root { Child value \"Other\" { x \"1\" } }").unwrap();
        assert_eq!(
            doc.path(&["root", "CHILD"]).and_then(Value::as_str),
            Some("value")
        );
        assert_eq!(
            doc.path(&["ROOT", "other", "X"]).and_then(Value::as_str),
            Some("1")
        );
    }
}
