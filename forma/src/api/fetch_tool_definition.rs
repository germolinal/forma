use agent::tool::fetch_tool::FetchTool;
use derive::FormaDocs;
use protocols::FormaError;
use serde::{Deserialize, Serialize};

use crate::api::ObjectDefinition;

/// Pulls the contents of a web page so that the agent has visibility of them. This includes
/// links so that the agent can navigate by fetching those links in turn.
///
/// ## Choosing what may be fetched
///
/// `allow_domains` and `block_domains` are lists of rules. A rule is a url,
/// optionally followed by a path, and it matches exactly what it says unless a
/// `*` widens it:
///
/// | Rule | Matches | Does not match |
/// | ---- | ------- | -------------- |
/// | `example.com` | `example.com`, at the root path | `example.com/pricing`, `docs.example.com` |
/// | `example.com/*` | `example.com`, `example.com/docs`, etc. | `docs.example.com`, `docs.example.com/a`, `differentwebsite.com/a` |
/// | `example.com/docs` | that one page | `example.com/docs/guide`, `example.com/` |
/// | `example.com/docs/*` | `example.com/docs` and everything under it | `example.com/pricing` |
/// | `*.example.com` | every subdomain, at its root path | `example.com` itself, `docs.example.com/guide` |
/// | `*.example.com/*` | any path on any subdomain | `example.com/guide` |
///
/// So `*` is never a wildcard for part of a name: it stands for whole subdomain
/// labels (`*.example.com`) or for whole path segments (`example.com/docs/*`).
/// A rule such as `foo.*.com` or `example.com/doc*` is rejected. To cover a
/// domain and its subdomains, list both:
///
/// ```text
/// allow_domains:
///   - example.com/*
///   - '*.example.com/*'
/// ```
///
/// Quote entries beginning with `*`, as above. Unquoted, YAML reads a leading
/// `*` as an alias and the file will not load.
///
/// ## How the two lists combine
///
/// A URL is fetched when `allow_domains` permits it *and* `block_domains` does
/// not match it, checked in that order. An empty `allow_domains` permits every
/// host, so a block-list on its own acts as a deny-list.
///
/// Because the block-list is applied to whatever the allow-list let through, a
/// block rule can be a subset of an allow rule. This is the usual way to open a
/// site up while keeping part of it out of reach:
///
/// ```text
/// allow_domains:
///   - '*.example.com/*'      # every subdomain, any path
/// block_domains:
///   - '*.example.com/admin/*' # ... except anything under /admin
///   - internal.example.com/*  # ... and this subdomain entirely
/// ```
///
/// Blocking something the allow-list never covered has no effect: it was
/// already refused for not being allowed.
///
/// ## Details worth knowing
///
/// - Hosts are matched case-insensitively; paths are case-sensitive, as on the web.
/// - A trailing slash is not significant: `example.com/docs` and `example.com/docs/` are the same page to a rule.
/// - A port, query string or fragment in the URL does not affect matching.
/// - The rules apply to every hop of a redirect, not only to the URL the agent asked for.
/// - Loopback and private-network addresses are always refused, whatever the rules say.
/// - Only web pages and text are read. A link to a PDF, a picture or any other file comes back as a note that it was not fetched, and none of it is downloaded.
/// - At most 2 MiB of a page is read. A longer one is cut there, and comes back marked as truncated.
#[derive(Clone, Debug, Serialize, Deserialize, Default, FormaDocs)]
pub struct FetchToolDefinition {
    /// If non-empty, only URLs matching one of these rules may be fetched. Empty
    /// means no allow-list: any host, subject to the block-list and to the
    /// loopback/private-network guard.
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

impl ObjectDefinition for FetchToolDefinition {
    fn expand(self, _parent_dir: &str) -> Result<Self, FormaError> {
        Ok(self)
    }
}

impl TryFrom<FetchToolDefinition> for FetchTool {
    type Error = FormaError;

    fn try_from(value: FetchToolDefinition) -> Result<Self, Self::Error> {
        Ok(Self {
            allow_domains: value.allow_domains,
            block_domains: value.block_domains,
            max_chars: value.max_chars,
            max_links: value.max_links,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent::ToolTrait;
    use db::NoneDB;

    /// The example from the docs has to load and pass validation, quoting and
    /// all: it is what readers will copy.
    #[test]
    fn documented_yaml_example_loads_and_validates() {
        let yaml = r#"
allow_domains:
  - '*.example.com/*'
  - example.com/*
  - example.com/docs
block_domains:
  - '*.example.com/admin/*'
  - internal.example.com/*
"#;
        let def: FetchToolDefinition = serde_yaml::from_str(yaml).expect("should deserialize");
        let tool = FetchTool::try_from(def).expect("should convert");
        <FetchTool as ToolTrait<NoneDB>>::check(&tool).expect("rules should be valid");
    }

    /// The docs tell readers to quote entries starting with `*`; this is why.
    #[test]
    fn unquoted_wildcard_entry_is_not_valid_yaml() {
        let yaml = "allow_domains:\n  - *.example.com/*\n";
        assert!(serde_yaml::from_str::<FetchToolDefinition>(yaml).is_err());
    }
}
