//! `FGameplayTagQuery` as data tables store it (`D_TagQueries` rows): a
//! token stream over a tag dictionary, evaluated against a tag set that
//! already holds every tag's parents (an item's `Generated_Tags` lists both).
//!
//! The stream is the engine's: a version byte, a has-root flag, then one
//! expression: its type (1 any, 2 all, 3 no tags match; 4 any, 5 all, 6 no
//! expressions match), a count and that many tag indices or nested
//! expressions.
use std::collections::BTreeSet;

use anyhow::{Context, Result, bail, ensure};

/// The deepest expression nesting accepted; data rows nest a few levels.
const MAX_DEPTH: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Node {
    AnyTags(Vec<String>),
    AllTags(Vec<String>),
    NoTags(Vec<String>),
    AnyExpr(Vec<Node>),
    AllExpr(Vec<Node>),
    NoExpr(Vec<Node>),
}

/// A compiled tag query; `None` is the empty query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagQuery {
    root: Option<Node>,
}

impl TagQuery {
    /// Compile a `D_TagQueries` row's `Query` (`QueryTokenStream`,
    /// `TagDictionary`).
    pub fn from_row(row: &serde_json::Value) -> Result<Self> {
        let query = row.get("Query").context("a tag query row has no Query")?;
        let tokens: Vec<u8> = query
            .get("QueryTokenStream")
            .and_then(serde_json::Value::as_array)
            .context("a tag query has no QueryTokenStream")?
            .iter()
            .map(|token| {
                token
                    .as_u64()
                    .and_then(|token| u8::try_from(token).ok())
                    .context("a tag query token is not a byte")
            })
            .collect::<Result<_>>()?;
        let dictionary: Vec<String> = match query.get("TagDictionary") {
            None => Vec::new(),
            Some(tags) => tags
                .as_array()
                .context("a tag query's TagDictionary is not an array")?
                .iter()
                .map(|tag| {
                    tag.get("TagName")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .context("a tag query tag has no TagName")
                })
                .collect::<Result<_>>()?,
        };
        let mut reader = Tokens {
            tokens: &tokens,
            at: 0,
            dictionary: &dictionary,
        };
        let root = match tokens.len() {
            0 => None,
            _ => {
                reader.next()?; // version
                if reader.next()? == 0 {
                    None
                } else {
                    Some(reader.expr(0)?)
                }
            }
        };
        ensure!(reader.at == tokens.len(), "a tag query has trailing tokens");
        Ok(Self { root })
    }

    /// Whether a tag set (with parents) satisfies the query; the empty
    /// query matches nothing, as the engine's does.
    pub fn matches(&self, tags: &BTreeSet<String>) -> bool {
        self.root.as_ref().is_some_and(|root| root.matches(tags))
    }
}

impl Node {
    fn matches(&self, tags: &BTreeSet<String>) -> bool {
        match self {
            Self::AnyTags(list) => list.iter().any(|tag| tags.contains(tag)),
            Self::AllTags(list) => list.iter().all(|tag| tags.contains(tag)),
            Self::NoTags(list) => !list.iter().any(|tag| tags.contains(tag)),
            Self::AnyExpr(list) => list.iter().any(|expr| expr.matches(tags)),
            Self::AllExpr(list) => list.iter().all(|expr| expr.matches(tags)),
            Self::NoExpr(list) => !list.iter().any(|expr| expr.matches(tags)),
        }
    }
}

struct Tokens<'a> {
    tokens: &'a [u8],
    at: usize,
    dictionary: &'a [String],
}

impl Tokens<'_> {
    fn next(&mut self) -> Result<u8> {
        let token = *self
            .tokens
            .get(self.at)
            .context("a tag query token stream ends early")?;
        self.at += 1;
        Ok(token)
    }

    fn expr(&mut self, depth: usize) -> Result<Node> {
        ensure!(depth < MAX_DEPTH, "a tag query nests too deeply");
        let kind = self.next()?;
        let count = usize::from(self.next()?);
        match kind {
            1..=3 => {
                let tags = (0..count)
                    .map(|_| {
                        let index = usize::from(self.next()?);
                        self.dictionary
                            .get(index)
                            .cloned()
                            .context("a tag query names a tag outside its dictionary")
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(match kind {
                    1 => Node::AnyTags(tags),
                    2 => Node::AllTags(tags),
                    _ => Node::NoTags(tags),
                })
            }
            4..=6 => {
                let exprs = (0..count)
                    .map(|_| self.expr(depth + 1))
                    .collect::<Result<Vec<_>>>()?;
                Ok(match kind {
                    4 => Node::AnyExpr(exprs),
                    5 => Node::AllExpr(exprs),
                    _ => Node::NoExpr(exprs),
                })
            }
            _ => bail!("unknown tag query expression type {kind}"),
        }
    }
}
