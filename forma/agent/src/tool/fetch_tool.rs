// A navigational web-fetch tool.
//
// Unlike the search tools (Tavily, Vertex AI Search), this tool takes a single
// URL, downloads the page, converts the rendered HTML into Markdown (cheaper and
// more readable for the LLM than raw HTML), and returns the list of links found
// on the page. The agent "clicks a link" simply by calling the tool again with
// one of those URLs, so link-following needs no state kept between calls.
//
// Which URLs may be fetched is fixed configuration on the tool: an optional
// allow-list and an optional block-list, both written as explicit host/path
// rules (see `DomainRule`). Loopback / private-network hosts are always refused
// (a minimal SSRF guard).
//
// Human-in-the-loop confirmation is not handled here: it is provided generically
// by `Tool::confirm_before_calling`, so an agent author can require "always ask
// me before fetching" without any tool-specific code.

use std::collections::HashMap;
use std::net::IpAddr;

use async_trait::async_trait;
use genai_client::{JsonSchema, schema::SchemaType};
use protocols::{FormaDB, FormaError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::{Host, Url};

use crate::{CallContext, ToolTrait, tool::mailgun_tool::string_schema};

#[cfg(not(feature = "airtight"))]
use tracing::{info, warn};

/// Default cap on how many characters of Markdown to return.
const DEFAULT_MAX_CHARS: usize = 20_000;
/// Default cap on how many links to return.
const DEFAULT_MAX_LINKS: usize = 100;
/// Request timeout, in seconds.
#[cfg(not(feature = "airtight"))]
const TIMEOUT_SECS: u64 = 20;
/// How many redirects to follow before giving up.
#[cfg(not(feature = "airtight"))]
const MAX_REDIRECTS: usize = 10;
/// User agent sent with requests.
#[cfg(not(feature = "airtight"))]
const USER_AGENT: &str = "FormaAgent/1.0 (+fetch tool)";
/// The most of a page's body that is read: 2 MiB. The answer is cut to
/// `max_chars` anyway, a page's HTML is seldom more than a megabyte or two,
/// and parsing it takes several times its size in memory, which every other
/// conversation on the server shares. A longer page is cut here, and the
/// result says it was truncated.
#[cfg(not(feature = "airtight"))]
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
/// The most of an error page that is read: only its start is ever shown.
#[cfg(not(feature = "airtight"))]
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

/// The client every fetch goes through, so connections to a site are kept
/// between calls. It follows no redirects by itself: each hop has to be vetted
/// against the rules of the tool that asked (see `FetchTool::get`), which a
/// client shared by every agent cannot know.
#[cfg(not(feature = "airtight"))]
static FETCH_CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("could not build the fetch tool's HTTP client")
});

/// Fetches a web page and returns it as Markdown plus the links it contains, so
/// the agent can navigate by fetching those links in turn.
///
/// `allow_domains` and `block_domains` are lists of rules. A rule is a host,
/// optionally followed by a path, and it matonhes exactly what it says unless a
/// `*` widens it:
///
/// | Rule                 | Matches                                                   |
/// | -------------------- | --------------------------------------------------------- |
/// | `example.com`        | that host at the root path, and nothing else                |
/// | `example.com/*`      | any path on that host                                       |
/// | `example.com/docs`   | that one page                                               |
/// | `example.com/docs/*` | `/docs` and everything under it                             |
/// | `*.example.com`      | every subdomain at its root path, but never `example.com`   |
/// | `*.example.com/*`    | any path on every subdomain                                 |
///
/// A URL is fetched when the allow-list permits it *and* the block-list does not
/// match it, checked in that order. An empty `allow_domains` permits every host,
/// so a block-list on its own acts as a deny-list. Because the block-list is
/// applied to whatever the allow-list let through, a block rule can be a subset
/// of an allow rule:
///
/// ```text
/// allow_domains:
///   - example.com/docs/*    # the docs, and
///   - '*.example.com/*'     # anything on any subdomain
/// block_domains:
///   - '*.example.com/admin/*'
/// ```
///
/// Quote entries beginning with `*` in YAML, as above: unquoted, YAML reads a
/// leading `*` as an alias. Hosts are matched case-insensitively, paths are not
/// (as on the web), and a port, query string or fragment in the URL does not
/// affect matching. The rules apply to every hop of a redirect, not only to the
/// URL the agent asked for.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct FetchTool {
    /// If non-empty, only URLs matching one of these rules may be fetched. Empty
    /// means no allow-list: any host, subject to the block-list and SSRF guard.
    #[serde(default)]
    pub allow_domains: Vec<String>,

    /// URLs matching one of these rules are refused, even when the allow-list
    /// permits them. Applied after `allow_domains`, so these rules can carve
    /// holes in it.
    #[serde(default)]
    pub block_domains: Vec<String>,

    /// Maximum number of Markdown characters to return; longer content is
    /// truncated. `None` uses [`DEFAULT_MAX_CHARS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_chars: Option<usize>,

    /// Maximum number of links to return. `None` uses [`DEFAULT_MAX_LINKS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_links: Option<usize>,
}

/// The single argument the LLM fills in: the URL to fetch.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct FetchRequest {
    url: String,
}

/// A link extracted from the page, with its (absolute) URL and visible text.
#[derive(Clone, Debug, Serialize, PartialEq)]
struct OutLink {
    text: String,
    url: String,
}

impl FetchTool {
    fn max_chars(&self) -> usize {
        self.max_chars.unwrap_or(DEFAULT_MAX_CHARS)
    }
    fn max_links(&self) -> usize {
        self.max_links.unwrap_or(DEFAULT_MAX_LINKS)
    }

    /// Validates the requested URL against the scheme rule, the SSRF guard and
    /// the configured domain lists. Returns the parsed URL on success.
    fn vet_url(&self, raw: &str) -> Result<Url, FormaError> {
        let url = Url::parse(raw)
            .map_err(|e| FormaError::Input(format!("FetchTool: invalid URL '{raw}': {e}")))?;

        if !matches!(url.scheme(), "http" | "https") {
            return Err(FormaError::Input(format!(
                "FetchTool: only http(s) URLs are allowed, got scheme '{}'",
                url.scheme()
            )));
        }

        let host = url
            .host()
            .ok_or_else(|| FormaError::Input(format!("FetchTool: URL '{raw}' has no host")))?;

        if host_is_private(&host) {
            return Err(FormaError::Input(format!(
                "FetchTool: refusing to fetch loopback/private-network host in '{raw}'"
            )));
        }

        let allow = parse_rules(&self.allow_domains, "allow_domains")?;
        let block = parse_rules(&self.block_domains, "block_domains")?;
        if let Err(why) = rules_permit(&host.to_string(), url.path(), &allow, &block) {
            return Err(FormaError::Input(format!(
                "FetchTool: refusing to fetch '{raw}': {why}"
            )));
        }

        Ok(url)
    }

    #[cfg(any(test, not(feature = "airtight")))]
    /// Where a response sends the fetch next, when it is a redirect: the
    /// `Location` it names, resolved against `current` and vetted like the URL
    /// the agent asked for. `None` when the response is the answer.
    fn redirect(
        &self,
        status: u16,
        location: Option<&str>,
        current: &Url,
    ) -> Result<Option<Url>, FormaError> {
        // The redirects a browser follows; 300 and 304 are answers.
        if !matches!(status, 301 | 302 | 303 | 307 | 308) {
            return Ok(None);
        }
        // One that names nowhere is the answer itself.
        let Some(location) = location else {
            return Ok(None);
        };
        let next = current.join(location).map_err(|e| {
            FormaError::Input(format!(
                "FetchTool: '{current}' redirects to '{location}', which is not a URL: {e}"
            ))
        })?;
        self.vet_url(next.as_str()).map(Some)
    }
}

/// One entry of `allow_domains` / `block_domains`, split into the host it names
/// and the paths it covers. See [`FetchTool`] for the syntax.
#[derive(Clone, Debug, PartialEq)]
struct DomainRule {
    /// The host, lowercased and without a trailing dot. For `*.example.com`
    /// this holds `example.com`, with `subdomains` recording the wildcard.
    host: String,
    /// Set by a leading `*.`: match strict subdomains of `host`, never `host`
    /// itself. Listing the apex as well takes a second rule.
    subdomains: bool,
    path: PathRule,
}

/// The path half of a [`DomainRule`].
#[derive(Clone, Debug, PartialEq)]
enum PathRule {
    /// The rule named no path, so only the root path matches.
    Root,
    /// The rule named a path and no `*`, so only that path matches.
    Exact(String),
    /// The rule ended in `/*`: the prefix itself and everything under it match.
    /// The prefix is empty for `example.com/*`, which covers every path.
    Under(String),
}

impl DomainRule {
    /// Parses one entry. The error explains what is wrong with the entry alone,
    /// so the caller can prefix it with the list the entry came from.
    fn parse(raw: &str) -> Result<Self, String> {
        let entry = raw.trim();
        if entry.is_empty() {
            return Err("entries must not be empty".to_string());
        }
        if entry.contains("://") {
            return Err(format!(
                "'{entry}' must not include a scheme; write the host on its own"
            ));
        }

        // Everything up to the first '/' is the host; the rest is the path.
        let (host, path) = match entry.split_once('/') {
            Some((host, path)) => (host, Some(path)),
            None => (entry, None),
        };

        let (host, subdomains) = match host.strip_prefix("*.") {
            Some(rest) => (rest, true),
            None => (host, false),
        };
        let host = host.trim_end_matches('.').to_ascii_lowercase();

        if host.is_empty() {
            return Err(format!("'{entry}' names no host"));
        }
        if host.contains('*') {
            return Err(format!(
                "'{entry}': '*' is only allowed as a leading '*.' label or a trailing '/*'"
            ));
        }
        if host.contains(':') {
            return Err(format!("'{entry}' must not include a port"));
        }
        if host.contains('@') {
            return Err(format!("'{entry}' must not include credentials"));
        }

        let path = match path {
            None => PathRule::Root,
            // Put back the separator `split_once` consumed.
            Some(rest) => match format!("/{rest}") {
                p if !p.contains('*') => PathRule::Exact(normalize_path(&p)),
                p => match p.strip_suffix("/*") {
                    Some(prefix) if !prefix.contains('*') => {
                        PathRule::Under(prefix.trim_end_matches('/').to_string())
                    }
                    _ => {
                        return Err(format!(
                            "'{entry}': '*' in a path is only allowed as a trailing '/*'"
                        ));
                    }
                },
            },
        };

        Ok(Self {
            host,
            subdomains,
            path,
        })
    }

    /// True when this rule covers `host` (as written in the URL) and `path`.
    fn matches(&self, host: &str, path: &str) -> bool {
        self.host_matches(host) && self.path_matches(path)
    }

    fn host_matches(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if self.subdomains {
            host.ends_with(&format!(".{}", self.host))
        } else {
            host == self.host
        }
    }

    fn path_matches(&self, path: &str) -> bool {
        let path = normalize_path(path);
        match &self.path {
            PathRule::Root => path == "/",
            PathRule::Exact(p) => path == *p,
            // The empty prefix comes from `host/*` and covers every path.
            PathRule::Under(p) => p.is_empty() || path == *p || path.starts_with(&format!("{p}/")),
        }
    }
}

/// Trims a path to the form the rules compare: no trailing slash, and `/` for
/// the root. This is what makes `/docs` and `/docs/` the same page to a rule.
fn normalize_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Parses a whole allow/block list, naming the field in any error.
fn parse_rules(list: &[String], field: &str) -> Result<Vec<DomainRule>, FormaError> {
    list.iter()
        .map(|raw| {
            DomainRule::parse(raw)
                .map_err(|why| FormaError::Input(format!("FetchTool: '{field}': {why}")))
        })
        .collect()
}

/// Applies the two lists in the documented order: the allow-list decides what
/// may be fetched at all, then the block-list carves holes in what it let
/// through. The error is the reason, ready to be shown to the agent.
fn rules_permit(
    host: &str,
    path: &str,
    allow: &[DomainRule],
    block: &[DomainRule],
) -> Result<(), String> {
    // An empty allow-list is "no allow-list", i.e. every host.
    if !allow.is_empty() && !allow.iter().any(|r| r.matches(host, path)) {
        return Err("this tool's allow-list does not cover that URL".to_string());
    }
    if block.iter().any(|r| r.matches(host, path)) {
        return Err("this tool's block-list covers that URL".to_string());
    }
    Ok(())
}

/// Minimal SSRF guard: refuse loopback / private / link-local / unspecified IPs,
/// and the literal `localhost`. This only inspects the requested URL's host, not
/// hosts reached through redirects.
fn host_is_private(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(d) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            d == "localhost" || d.ends_with(".localhost")
        }
        Host::Ipv4(ip) => ip_is_private(&IpAddr::V4(*ip)),
        Host::Ipv6(ip) => ip_is_private(&IpAddr::V6(*ip)),
    }
}

fn ip_is_private(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                // Unique-local (fc00::/7) and link-local (fe80::/10). `is_unique_local`
                // / `is_unicast_link_local` are unstable, so match the prefixes directly.
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

#[cfg(any(test, not(feature = "airtight")))]
/// How a response is read, decided from its `Content-Type` before any of its
/// body is.
#[derive(Debug, PartialEq, Eq)]
enum BodyKind {
    /// Turned into Markdown and links. A response that names no type is taken
    /// for a page.
    Html,
    /// Returned as it is.
    Text,
    /// Not read at all: only text is ever returned, so downloading a PDF or a
    /// video would be for nothing.
    Other,
}

#[cfg(any(test, not(feature = "airtight")))]
impl BodyKind {
    fn of(content_type: &str) -> Self {
        if content_type.contains("html") || content_type.is_empty() {
            Self::Html
        } else if content_type.starts_with("text/") {
            Self::Text
        } else {
            Self::Other
        }
    }
}

#[cfg(any(test, not(feature = "airtight")))]
/// Reads `body` until it ends or `limit` bytes have arrived, and says whether
/// it was cut there. Whatever comes after the limit is never read.
async fn read_capped<E>(
    body: impl futures::Stream<Item = Result<bytes::Bytes, E>>,
    limit: usize,
) -> Result<(Vec<u8>, bool), E> {
    use futures::StreamExt;

    let mut body = std::pin::pin!(body);
    let mut read = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        let room = limit - read.len();
        if chunk.len() > room {
            read.extend_from_slice(&chunk[..room]);
            return Ok((read, true));
        }
        read.extend_from_slice(&chunk);
    }
    Ok((read, false))
}

#[cfg(any(test, not(feature = "airtight")))]
/// The `charset` a `Content-Type` names, if it names one.
fn charset(content_type: &str) -> Option<&str> {
    content_type.split(';').skip(1).find_map(|param| {
        let (name, value) = param.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches('"'))
    })
}

#[cfg(any(test, not(feature = "airtight")))]
/// `bytes` as text, in the charset `content_type` names, or UTF-8 when it
/// names none: what `reqwest::Response::text` does. A character that the byte
/// limit cut in two comes out as a replacement character.
fn decode(bytes: &[u8], content_type: &str) -> String {
    let encoding = charset(content_type)
        .and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    encoding.decode(bytes).0.into_owned()
}

/// The Markdown + links extracted from a fetched HTML document.
struct Extracted {
    title: Option<String>,
    markdown: String,
    truncated: bool,
    links: Vec<OutLink>,
}

/// Returns a human-readable explanation for status codes that indicate the site
/// is blocking automated access (login / anti-bot walls) rather than a genuine
/// per-request error. This gives the agent something actionable instead of an
/// opaque code, and signals that retrying the same URL won't help.
///
/// `999` is non-standard — LinkedIn (and a few others) return it to
/// unauthenticated scrapers. `401`/`403`/`451` are the standard auth/blocked
/// responses. For these the response body is usually a login page or challenge,
/// so it isn't worth surfacing.
fn auth_wall_hint(status: u16) -> Option<&'static str> {
    match status {
        999 => Some(
            "the site is blocking automated access (a login/anti-bot wall, \
             e.g. LinkedIn returns this to non-authenticated requests); the \
             page cannot be fetched without signing in — retrying won't help",
        ),
        401 | 403 => Some(
            "access is denied; the page likely requires signing in or is \
             behind an anti-bot wall — retrying the same URL won't help",
        ),
        451 => Some("the page is unavailable for legal reasons in this region"),
        _ => None,
    }
}

/// Collapses runs of whitespace into single spaces and trims.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Strips `<template ...>` / `</template>` tags while keeping their inner
/// content. Per the HTML spec a `<template>`'s children live in an inert
/// document fragment, so html-to-markdown converters (like htmd) emit nothing
/// for them. Framework-rendered pages (e.g. Vue SSR) wrap their real content in
/// `<template>` slots, which would otherwise convert to an empty document.
/// Promoting the content into the normal DOM lets it be converted.
fn unwrap_templates(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while i < html.len() {
        let rest = &lower[i..];
        if rest.starts_with("<template") || rest.starts_with("</template") {
            // Drop the whole tag up to and including its closing '>'.
            match html[i..].find('>') {
                Some(gt) => {
                    i += gt + 1;
                    continue;
                }
                None => break,
            }
        }
        let ch = html[i..].chars().next().expect("i is on a char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Truncates `s` to at most `max` characters (never splitting a char).
fn truncate_chars(s: &str, max: usize) -> (String, bool) {
    if s.chars().count() <= max {
        (s.to_string(), false)
    } else {
        (s.chars().take(max).collect(), true)
    }
}

/// Parses HTML: converts the body to Markdown and pulls out the (absolute)
/// links. `base` is the page's final URL, used to resolve relative hrefs.
fn extract_html(html: &str, base: &Url, max_chars: usize, max_links: usize) -> Extracted {
    use scraper::{Html, Selector};

    let doc = Html::parse_document(html);

    let title = Selector::parse("title").ok().and_then(|sel| {
        doc.select(&sel)
            .next()
            .map(|el| collapse_ws(&el.text().collect::<String>()))
            .filter(|t| !t.is_empty())
    });

    let mut links: Vec<OutLink> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Ok(sel) = Selector::parse("a[href]") {
        for el in doc.select(&sel) {
            if links.len() >= max_links {
                break;
            }
            let Some(href) = el.value().attr("href") else {
                continue;
            };
            let href = href.trim();
            if href.is_empty() || href.starts_with('#') {
                continue;
            }
            // Resolve relative to the page, then keep only navigable http(s) links.
            let Ok(abs) = base.join(href) else { continue };
            if !matches!(abs.scheme(), "http" | "https") {
                continue;
            }
            let abs = abs.to_string();
            if !seen.insert(abs.clone()) {
                continue;
            }
            links.push(OutLink {
                text: collapse_ws(&el.text().collect::<String>()),
                url: abs,
            });
        }
    }

    // html-to-markdown. Promote <template> content into the DOM (see
    // `unwrap_templates`), then skip non-content tags so their contents (CSS,
    // inline JS, the <title>, etc.) don't leak into the Markdown. On the rare
    // conversion error, fall back to empty text rather than failing the whole
    // fetch (the links are still useful).
    //
    // NB: skipping "head" is intentionally avoided — it makes htmd emit an
    // empty document on some pages. Skipping "title" is enough to keep the
    // (separately-extracted) title out of the body text.
    let prepared = unwrap_templates(html);
    let md = htmd::HtmlToMarkdown::builder()
        .skip_tags(vec![
            "script", "style", "noscript", "svg", "iframe", "title",
        ])
        .build()
        .convert(&prepared)
        .unwrap_or_default();
    let (markdown, truncated) = truncate_chars(&md, max_chars);

    Extracted {
        title,
        markdown,
        truncated,
        links,
    }
}

impl TryFrom<FetchTool> for JsonSchema {
    type Error = FormaError;

    fn try_from(_item: FetchTool) -> Result<JsonSchema, FormaError> {
        // Domain rules and limits are fixed tool configuration, so the only
        // thing the LLM supplies is the URL.
        let mut props = HashMap::new();
        props.insert(
            "url".to_string(),
            string_schema("the absolute http(s) URL of the page to fetch"),
        );
        Ok(JsonSchema {
            schema_type: Some(SchemaType::Object),
            properties: Some(props),
            required: Some(vec!["url".to_string()]),
            additional_properties: false,
            ..Default::default()
        })
    }
}

#[async_trait]
impl<DB: FormaDB> ToolTrait<DB> for FetchTool {
    fn check(&self) -> Result<(), FormaError> {
        // Report a malformed rule when the agent is deployed rather than when it
        // first tries to fetch something.
        parse_rules(&self.allow_domains, "allow_domains")?;
        parse_rules(&self.block_domains, "block_domains")?;
        if self.max_chars == Some(0) {
            return Err(FormaError::Input(
                "FetchTool: 'max_chars' must be greater than 0".to_string(),
            ));
        }
        Ok(())
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: CallContext<DB>,
    ) -> Result<Option<Value>, FormaError> {
        let req: FetchRequest = serde_json::from_value(input)
            .map_err(|e| FormaError::Input(format!("FetchTool: invalid input: {e}")))?;

        let url = self.vet_url(&req.url)?;

        #[cfg(not(feature = "airtight"))]
        {
            info!(url = %url, "fetching page");
            // One deadline for the whole fetch: every redirect, and the body.
            let deadline = std::time::Duration::from_secs(TIMEOUT_SECS);
            tokio::time::timeout(deadline, self.fetch(url.clone()))
                .await
                .map_err(|_| FormaError::ApiCall {
                    url: Some(url.to_string()),
                    msg: format!("the page did not arrive within {TIMEOUT_SECS} seconds"),
                })?
        }
        #[cfg(feature = "airtight")]
        {
            // Airtight builds make no network calls.
            let _ = url;
            Ok(Some(Value::Null))
        }
    }
}

#[cfg(not(feature = "airtight"))]
impl FetchTool {
    /// Requests `url`, following its redirects one hop at a time so that each
    /// is vetted like the URL the agent asked for: an allowed page that
    /// redirects elsewhere must not be a way around the rules, or around the
    /// guard on private addresses.
    async fn get(&self, mut url: Url) -> Result<reqwest::Response, FormaError> {
        let mut hops = 0;
        loop {
            let resp = FETCH_CLIENT.get(url.clone()).send().await?;
            let location = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok());
            match self.redirect(resp.status().as_u16(), location, &url)? {
                None => return Ok(resp),
                Some(_) if hops == MAX_REDIRECTS => {
                    return Err(FormaError::ApiCall {
                        url: Some(url.to_string()),
                        msg: format!("more than {MAX_REDIRECTS} redirects"),
                    });
                }
                Some(next) => {
                    hops += 1;
                    url = next;
                }
            }
        }
    }

    /// Fetches the page at `url` and turns it into what the agent is given.
    async fn fetch(&self, url: Url) -> Result<Option<Value>, FormaError> {
        let resp = self.get(url.clone()).await?;
        let status = resp.status();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !status.is_success() {
            let body = read_capped(resp.bytes_stream(), MAX_ERROR_BODY_BYTES)
                .await
                .map(|(bytes, _)| decode(&bytes, &content_type))
                .unwrap_or_default();
            warn!(url = %url, %status, "fetch returned error");
            return Err(FormaError::ApiCall {
                url: Some(url.to_string()),
                msg: auth_wall_hint(status.as_u16())
                    .map(|hint| format!("HTTP {status}: {hint}"))
                    .unwrap_or_else(|| format!("HTTP {status}: {}", truncate_chars(&body, 500).0)),
            });
        }

        // The final URL after any redirects; used to resolve relative links.
        let final_url = resp.url().clone();
        let kind = BodyKind::of(&content_type);
        // Non-textual content (PDF, image, …): reported without reading it.
        if kind == BodyKind::Other {
            return Ok(Some(json!({
                "url": final_url.to_string(),
                "content_type": content_type,
                "note": "non-text content was not fetched",
            })));
        }
        let (bytes, cut) = read_capped(resp.bytes_stream(), MAX_BODY_BYTES).await?;
        let text = decode(&bytes, &content_type);

        if kind == BodyKind::Html {
            // Parsing a page is CPU work that grows with it: done in place,
            // it would hold up every other run sharing the thread.
            let (base, max_chars, max_links) =
                (final_url.clone(), self.max_chars(), self.max_links());
            let ex = tokio::task::spawn_blocking(move || {
                extract_html(&text, &base, max_chars, max_links)
            })
            .await
            .map_err(|e| FormaError::Execution(format!("could not read the page: {e}")))?;
            return Ok(Some(json!({
                "url": final_url.to_string(),
                "title": ex.title,
                "markdown": ex.markdown,
                "truncated": ex.truncated || cut,
                "links": ex.links,
            })));
        }
        let (body, truncated) = truncate_chars(&text, self.max_chars());
        Ok(Some(json!({
            "url": final_url.to_string(),
            "content_type": content_type,
            "text": body,
            "truncated": truncated || cut,
            "links": [],
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::callable::CallContext;
    use db::NoneDB;
    use protocols::{DeploymentCredentials, EncryptedTokens, SessionData};
    use std::sync::Arc;
    use tokio::sync::{Mutex, RwLock, mpsc};

    fn make_ctx() -> CallContext<NoneDB> {
        let (tx, _rx) = mpsc::unbounded_channel::<genai_client::SSEvent>();
        CallContext {
            user_timezone: None,
            agent_timezone: None,
            session: Arc::new(RwLock::new(SessionData::default())),
            tx,
            job_id: "test".into(),
            user_roles: vec![].into(),
            user_credentials: Arc::new(Mutex::new(EncryptedTokens::default())),
            db: Arc::new(NoneDB::default()),
            user_id: "user_id".into(),
            deployment_credentials: Arc::new(DeploymentCredentials::default()),
            agent_id: "test-agent".into(),
            deployment_id: None,
        }
    }

    // ── rule matching ─────────────────────────────────────────────────────────

    /// Asserts that `rule` matches exactly the URLs in `hits` and none of those
    /// in `misses`, so each case reads as the rule's whole contract.
    fn assert_rule(rule: &str, hits: &[&str], misses: &[&str]) {
        let rule = DomainRule::parse(rule).expect("rule should parse");
        for url in hits {
            let url = Url::parse(url).unwrap();
            assert!(
                rule.matches(&url.host().unwrap().to_string(), url.path()),
                "{rule:?} should match {url}"
            );
        }
        for url in misses {
            let url = Url::parse(url).unwrap();
            assert!(
                !rule.matches(&url.host().unwrap().to_string(), url.path()),
                "{rule:?} should not match {url}"
            );
        }
    }

    #[test]
    fn bare_host_rule_is_that_host_at_the_root() {
        assert_rule(
            "example.com",
            &[
                "https://example.com",
                "https://example.com/",
                "http://example.com/?q=1",
                "https://EXAMPLE.com/",
                "https://example.com./",
            ],
            &[
                "https://example.com/docs",
                "https://docs.example.com/",
                "https://notexample.com/",
                "https://example.com.evil.com/",
            ],
        );
    }

    #[test]
    fn host_wildcard_covers_every_path_on_that_host_only() {
        assert_rule(
            "example.com/*",
            &[
                "https://example.com/",
                "https://example.com/docs",
                "https://example.com/a/b/c?q=1#frag",
            ],
            &[
                "https://docs.example.com/a",
                "https://differentwebsite.com/a",
            ],
        );
    }

    #[test]
    fn exact_path_rule_matches_only_that_page() {
        assert_rule(
            "example.com/docs",
            &["https://example.com/docs", "https://example.com/docs/"],
            &[
                "https://example.com/",
                "https://example.com/docs/guide",
                "https://example.com/docs2",
            ],
        );
    }

    #[test]
    fn path_wildcard_covers_the_prefix_and_everything_under_it() {
        assert_rule(
            "example.com/docs/*",
            &[
                "https://example.com/docs",
                "https://example.com/docs/",
                "https://example.com/docs/guide/intro",
            ],
            &[
                "https://example.com/",
                "https://example.com/docsomething",
                "https://docs.example.com/docs/guide",
            ],
        );
    }

    #[test]
    fn subdomain_wildcard_never_matches_the_apex() {
        assert_rule(
            "*.example.com",
            &["https://docs.example.com/", "https://a.b.example.com/"],
            &[
                "https://example.com/",
                "https://docs.example.com/guide",
                "https://example.com.evil.com/",
            ],
        );
        assert_rule(
            "*.example.com/*",
            &[
                "https://docs.example.com/guide",
                "https://a.b.example.com/x",
            ],
            &["https://example.com/guide"],
        );
    }

    #[test]
    fn paths_are_case_sensitive_but_hosts_are_not() {
        assert_rule(
            "example.com/Docs/*",
            &["https://EXAMPLE.COM/Docs/x"],
            &["https://example.com/docs/x"],
        );
    }

    // ── rule parsing ──────────────────────────────────────────────────────────

    #[test]
    fn parse_rejects_malformed_entries() {
        for bad in [
            "",
            "   ",
            "https://example.com", // scheme
            "example.com:8080",    // port
            "user@example.com",    // credentials
            "*",                   // bare wildcard
            "*.",                  // no host
            "foo.*.com",           // wildcard mid-host
            "example.com/*/docs",  // wildcard mid-path
            "example.com/docs*",   // partial-segment wildcard
        ] {
            assert!(
                DomainRule::parse(bad).is_err(),
                "'{bad}' should be rejected"
            );
        }
    }

    #[test]
    fn parse_accepts_the_documented_forms() {
        for good in [
            "example.com",
            "example.com/",
            "example.com/*",
            "example.com/docs",
            "example.com/docs/*",
            "*.example.com",
            "*.example.com/*",
            "  example.com/docs  ",
        ] {
            assert!(DomainRule::parse(good).is_ok(), "'{good}' should parse");
        }
    }

    // ── allow / block interaction ─────────────────────────────────────────────

    /// Applies both lists the way `vet_url` does, for a URL given as a string.
    fn permits(url: &str, allow: &[&str], block: &[&str]) -> bool {
        let url = Url::parse(url).unwrap();
        let parse = |list: &[&str]| {
            list.iter()
                .map(|r| DomainRule::parse(r).unwrap())
                .collect::<Vec<_>>()
        };
        rules_permit(
            &url.host().unwrap().to_string(),
            url.path(),
            &parse(allow),
            &parse(block),
        )
        .is_ok()
    }

    #[test]
    fn block_carves_a_hole_in_the_allow_list() {
        let allow = ["*.example.com/*"];
        let block = ["secret.example.com/*"];
        assert!(permits("https://docs.example.com/a", &allow, &block));
        assert!(!permits("https://secret.example.com/a", &allow, &block));
    }

    #[test]
    fn block_can_be_a_subset_of_a_single_allow_rule() {
        let allow = ["example.com/*"];
        let block = ["example.com/admin/*"];
        assert!(permits("https://example.com/docs", &allow, &block));
        assert!(!permits("https://example.com/admin", &allow, &block));
        assert!(!permits("https://example.com/admin/users", &allow, &block));
    }

    #[test]
    fn allow_list_is_checked_before_block_list() {
        // Blocking something the allow-list never covered changes nothing: the
        // URL was already refused for not being allowed.
        assert!(!permits("https://other.com/", &["example.com"], &[]));
        assert!(!permits(
            "https://other.com/",
            &["example.com"],
            &["other.com"]
        ));
    }

    #[test]
    fn empty_allow_permits_any_unblocked_url() {
        assert!(permits("https://anything.org/deep/path", &[], &[]));
        assert!(!permits("https://bad.org/", &[], &["bad.org"]));
        assert!(permits("https://bad.org/ok", &[], &["bad.org"]));
    }

    // ── SSRF guard ────────────────────────────────────────────────────────────

    fn tool() -> FetchTool {
        FetchTool::default()
    }

    #[test]
    fn rejects_localhost_and_private_ips() {
        assert!(tool().vet_url("http://localhost/x").is_err());
        assert!(tool().vet_url("http://127.0.0.1/x").is_err());
        assert!(tool().vet_url("http://10.0.0.5/x").is_err());
        assert!(tool().vet_url("http://192.168.1.1/x").is_err());
        assert!(
            tool()
                .vet_url("http://169.254.169.254/latest/meta-data")
                .is_err()
        );
        assert!(tool().vet_url("http://[::1]/x").is_err());
    }

    #[test]
    fn rejects_non_http_scheme() {
        assert!(tool().vet_url("ftp://example.com/x").is_err());
        assert!(tool().vet_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn accepts_public_host() {
        assert!(tool().vet_url("https://example.com/path").is_ok());
    }

    #[test]
    fn enforces_configured_domains() {
        let t = FetchTool {
            allow_domains: vec!["example.com/docs/*".to_string()],
            block_domains: vec!["example.com/docs/secret/*".to_string()],
            ..Default::default()
        };
        assert!(t.vet_url("https://example.com/docs/guide").is_ok());
        assert!(t.vet_url("https://example.com/docs/secret/keys").is_err());
        // Neither a subdomain nor another path on the same host is covered.
        assert!(t.vet_url("https://docs.example.com/docs/guide").is_err());
        assert!(t.vet_url("https://example.com/pricing").is_err());
        assert!(t.vet_url("https://other.com/docs/guide").is_err());
    }

    #[test]
    fn vet_url_reports_a_malformed_rule() {
        let t = FetchTool {
            allow_domains: vec!["https://example.com".to_string()],
            ..Default::default()
        };
        assert!(t.vet_url("https://example.com/").is_err());
    }

    // ── extraction ────────────────────────────────────────────────────────────

    #[test]
    fn extracts_title_markdown_and_absolute_links() {
        let base = Url::parse("https://example.com/dir/page").unwrap();
        let html = r##"
            <html><head><title>  Hello   World </title></head>
            <body>
              <h1>Heading</h1>
              <p>Some text with a <a href="/other">relative link</a> and an
                 <a href="https://elsewhere.org/x">absolute one</a>.</p>
              <a href="#frag">fragment only</a>
              <a href="mailto:a@b.com">email</a>
              <a href="/other">dup</a>
            </body></html>
        "##;
        let ex = extract_html(html, &base, 10_000, 100);
        assert_eq!(ex.title.as_deref(), Some("Hello World"));
        assert!(ex.markdown.contains("Heading"));
        assert!(!ex.truncated);
        // fragment, mailto and the duplicate are dropped; relative is resolved.
        assert_eq!(
            ex.links,
            vec![
                OutLink {
                    text: "relative link".to_string(),
                    url: "https://example.com/other".to_string(),
                },
                OutLink {
                    text: "absolute one".to_string(),
                    url: "https://elsewhere.org/x".to_string(),
                },
            ]
        );
    }

    #[test]
    fn strips_script_and_style_from_markdown() {
        let base = Url::parse("https://example.com/").unwrap();
        let html = r##"
            <html><head>
              <style>.foo { color: #123; } /* a comment */</style>
              <script>var x = {"a": 1}; console.log(x);</script>
            </head>
            <body>
              <noscript>enable javascript</noscript>
              <p>Visible content.</p>
              <script type="application/json">{"leak": true}</script>
            </body></html>
        "##;
        let ex = extract_html(html, &base, 10_000, 100);
        assert!(ex.markdown.contains("Visible content."));
        assert!(!ex.markdown.contains("color"));
        assert!(!ex.markdown.contains("console.log"));
        assert!(!ex.markdown.contains("leak"));
        assert!(!ex.markdown.contains("enable javascript"));
    }

    #[test]
    fn recovers_content_wrapped_in_templates() {
        // Framework-rendered pages (e.g. Vue SSR) put their real content inside
        // <template> slots and custom elements. htmd emits nothing for
        // <template> content, so extraction must unwrap those tags first.
        let base = Url::parse("https://example.com/").unwrap();
        let html = r##"
            <html><head><title>T</title></head>
            <body>
              <app :sticky="true">
                <template #header>
                  <h1>Real Heading</h1>
                  <p>Real paragraph text.</p>
                </template>
              </app>
            </body></html>
        "##;
        let ex = extract_html(html, &base, 10_000, 100);
        assert!(
            ex.markdown.contains("Real Heading"),
            "template content missing: {:?}",
            ex.markdown
        );
        assert!(ex.markdown.contains("Real paragraph text."));
    }

    #[test]
    fn auth_wall_statuses_get_a_hint() {
        // LinkedIn's non-standard anti-bot code and the standard auth/blocked
        // codes get an explanatory message; ordinary errors don't.
        assert!(auth_wall_hint(999).unwrap().contains("login"));
        assert!(auth_wall_hint(403).is_some());
        assert!(auth_wall_hint(401).is_some());
        assert!(auth_wall_hint(451).is_some());
        assert!(auth_wall_hint(404).is_none());
        assert!(auth_wall_hint(500).is_none());
    }

    #[test]
    fn markdown_is_truncated_to_max_chars() {
        let base = Url::parse("https://example.com/").unwrap();
        let html = format!("<p>{}</p>", "a".repeat(500));
        let ex = extract_html(&html, &base, 100, 10);
        assert!(ex.truncated);
        assert!(ex.markdown.chars().count() <= 100);
    }

    #[test]
    fn links_are_capped() {
        let base = Url::parse("https://example.com/").unwrap();
        let mut html = String::from("<body>");
        for i in 0..50 {
            html.push_str(&format!("<a href=\"/p{i}\">l{i}</a>"));
        }
        html.push_str("</body>");
        let ex = extract_html(&html, &base, 10_000, 5);
        assert_eq!(ex.links.len(), 5);
    }

    // ── reading the body ──────────────────────────────────────────────────────

    #[test]
    fn the_content_type_decides_what_is_read() {
        assert_eq!(BodyKind::of("text/html; charset=utf-8"), BodyKind::Html);
        assert_eq!(BodyKind::of("application/xhtml+xml"), BodyKind::Html);
        assert_eq!(BodyKind::of(""), BodyKind::Html);
        assert_eq!(BodyKind::of("text/plain"), BodyKind::Text);
        assert_eq!(BodyKind::of("text/csv; charset=utf-8"), BodyKind::Text);
        assert_eq!(BodyKind::of("application/pdf"), BodyKind::Other);
        assert_eq!(BodyKind::of("image/png"), BodyKind::Other);
        assert_eq!(BodyKind::of("video/mp4"), BodyKind::Other);
    }

    fn body(
        chunks: Vec<Result<&'static [u8], &'static str>>,
    ) -> impl futures::Stream<Item = Result<bytes::Bytes, &'static str>> {
        futures::stream::iter(chunks.into_iter().map(|c| c.map(bytes::Bytes::from_static)))
    }

    #[tokio::test]
    async fn a_body_is_read_up_to_the_limit() {
        let whole = || body(vec![Ok(b"hello "), Ok(b"world"), Ok(b"!!!")]);
        assert_eq!(
            read_capped(whole(), 100).await.unwrap(),
            (b"hello world!!!".to_vec(), false)
        );
        // Exactly the limit is the whole body, not a cut one.
        assert_eq!(
            read_capped(whole(), 14).await.unwrap(),
            (b"hello world!!!".to_vec(), false)
        );
        assert_eq!(
            read_capped(whole(), 8).await.unwrap(),
            (b"hello wo".to_vec(), true)
        );
    }

    #[tokio::test]
    async fn nothing_past_the_limit_is_read() {
        // Reading on would reach the error.
        let long = body(vec![Ok(b"12345678"), Err("read past the limit")]);
        assert_eq!(
            read_capped(long, 4).await.unwrap(),
            (b"1234".to_vec(), true)
        );
    }

    #[test]
    fn a_page_is_decoded_in_the_charset_it_names() {
        let latin1 = b"Se\xf1al";
        assert_eq!(decode(latin1, "text/html; charset=iso-8859-1"), "Señal");
        assert_eq!(decode(latin1, "text/html; charset=\"ISO-8859-1\""), "Señal");
        assert_eq!(decode("Señal".as_bytes(), "text/html"), "Señal");
        // Unnamed or unknown, it is read as UTF-8.
        assert_eq!(decode(latin1, "text/html"), "Se\u{fffd}al");
        assert_eq!(
            decode(latin1, "text/html; charset=nonsense"),
            "Se\u{fffd}al"
        );
    }

    // ── redirects ─────────────────────────────────────────────────────────────

    #[test]
    fn a_redirect_is_followed_to_where_it_points() {
        let docs = FetchTool {
            allow_domains: vec!["example.com/*".to_string()],
            ..Default::default()
        };
        let here = Url::parse("https://example.com/old").unwrap();
        for status in [301, 302, 303, 307, 308] {
            let next = docs.redirect(status, Some("/new"), &here).unwrap();
            assert_eq!(next.unwrap().as_str(), "https://example.com/new");
        }
        // Absolute, too.
        let next = docs
            .redirect(301, Some("https://example.com/elsewhere"), &here)
            .unwrap();
        assert_eq!(next.unwrap().as_str(), "https://example.com/elsewhere");
    }

    #[test]
    fn what_is_not_a_redirect_is_the_answer() {
        let here = Url::parse("https://example.com/").unwrap();
        for status in [200, 300, 304, 404] {
            assert_eq!(tool().redirect(status, Some("/x"), &here).unwrap(), None);
        }
        // A redirect that names nowhere.
        assert_eq!(tool().redirect(302, None, &here).unwrap(), None);
    }

    #[test]
    fn a_redirect_is_held_to_the_rules() {
        let docs = FetchTool {
            allow_domains: vec!["example.com/*".to_string()],
            block_domains: vec!["example.com/admin/*".to_string()],
            ..Default::default()
        };
        let here = Url::parse("https://example.com/").unwrap();
        // Out of the allow-list, into the block-list, onto a private address.
        for location in [
            "https://other.com/",
            "/admin/users",
            "http://127.0.0.1/",
            "http://10.0.0.5/",
        ] {
            assert!(
                docs.redirect(302, Some(location), &here).is_err(),
                "followed a redirect to {location}"
            );
        }
        // Nor anything but http(s).
        assert!(
            tool()
                .redirect(302, Some("file:///etc/passwd"), &here)
                .is_err()
        );
    }

    // ── check ─────────────────────────────────────────────────────────────────

    #[test]
    fn check_rejects_empty_domain_entry() {
        let t = FetchTool {
            block_domains: vec!["  ".to_string()],
            ..Default::default()
        };
        assert!(<FetchTool as ToolTrait<NoneDB>>::check(&t).is_err());
    }

    #[test]
    fn check_rejects_malformed_rules_in_either_list() {
        let allow = FetchTool {
            allow_domains: vec!["foo.*.com".to_string()],
            ..Default::default()
        };
        assert!(<FetchTool as ToolTrait<NoneDB>>::check(&allow).is_err());

        let block = FetchTool {
            block_domains: vec!["example.com:8080".to_string()],
            ..Default::default()
        };
        assert!(<FetchTool as ToolTrait<NoneDB>>::check(&block).is_err());
    }

    #[test]
    fn check_rejects_zero_max_chars() {
        let t = FetchTool {
            max_chars: Some(0),
            ..Default::default()
        };
        assert!(<FetchTool as ToolTrait<NoneDB>>::check(&t).is_err());
    }

    // ── schema ────────────────────────────────────────────────────────────────

    #[test]
    fn schema_only_takes_url() {
        let schema = JsonSchema::try_from(tool()).unwrap();
        assert_eq!(schema.schema_type, Some(SchemaType::Object));
        let props = schema.properties.unwrap();
        assert_eq!(props["url"].schema_type, Some(SchemaType::String));
        assert_eq!(schema.required.unwrap(), vec!["url".to_string()]);
    }

    // ── execute (input validation, no network) ────────────────────────────────

    #[tokio::test]
    async fn execute_rejects_bad_url_before_network() {
        let result = tool()
            .execute(json!({ "url": "not a url" }), make_ctx())
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn execute_rejects_blocked_host_before_network() {
        let result = tool()
            .execute(json!({ "url": "http://localhost:8080/admin" }), make_ctx())
            .await;
        assert!(result.is_err());
    }

    // Live network check; run explicitly with `cargo test -p agent -- --ignored`.
    #[cfg(not(feature = "airtight"))]
    #[tokio::test]
    #[ignore]
    async fn execute_live_fetch_example_com() {
        let result = tool()
            .execute(json!({ "url": "https://example.com/" }), make_ctx())
            .await
            .expect("fetch failed")
            .expect("no value");
        // The page's heading is gone; its title is what still names it.
        assert_eq!(result["title"], "Example Domain", "result was: {result}");
        let md = result["markdown"].as_str().unwrap_or_default();
        assert!(md.contains("documentation examples"), "markdown was: {md}");
        assert_eq!(result["truncated"], false);
        assert!(result["links"].is_array());
    }

    // Live network check, like the one above: w3.org redirects to www.w3.org.
    #[cfg(not(feature = "airtight"))]
    #[tokio::test]
    #[ignore]
    async fn execute_live_fetch_follows_a_redirect() {
        let result = tool()
            .execute(json!({ "url": "https://w3.org/" }), make_ctx())
            .await
            .expect("fetch failed")
            .expect("no value");
        assert_eq!(result["url"], "https://www.w3.org/", "result was: {result}");
        let md = result["markdown"].as_str().unwrap_or_default();
        assert!(!md.is_empty(), "result was: {result}");
    }

    #[cfg(feature = "airtight")]
    #[tokio::test]
    async fn execute_valid_returns_null_in_airtight() {
        let result = tool()
            .execute(json!({ "url": "https://example.com/" }), make_ctx())
            .await;
        assert_eq!(result.unwrap(), Some(Value::Null));
    }
}
