use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use serde::Deserialize;
use tokio::sync::OnceCell;
use tracing::{debug, info, warn};

use crate::models::{OnlineMediaProvider, ProviderListEntry, ProviderListResponse};

const GENERATED_PROVIDER_CATALOG_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../types/config/online-media-provider-catalog.json"
));

const YTDLP_DOMAINS_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/config/ytdlp-domains.json"
));

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCatalogEntry {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub source_site: String,
    pub supported_content_types: Vec<String>,
    pub default_content_type: String,
    pub requires_auth: bool,
    #[serde(default)]
    pub auth_configurable: bool,
    pub host_suffixes: Vec<String>,
    pub host_equals: Vec<String>,
    pub extractor_keywords: Vec<String>,
    #[serde(default)]
    pub source_site_aliases: Vec<String>,
    #[serde(default)]
    pub common_source_sites: Vec<String>,
}

impl ProviderCatalogEntry {
    pub fn to_provider(&self) -> OnlineMediaProvider {
        OnlineMediaProvider {
            id: self.id.clone(),
            name: self.name.clone(),
            display_name: Some(self.display_name.clone()),
            supported_content_types: self
                .supported_content_types
                .iter()
                .map(ToString::to_string)
                .collect(),
            requires_auth: self.requires_auth,
        }
    }

    pub fn matches_host(&self, host: &str) -> bool {
        self.host_equals.iter().any(|value| host == value)
            || self.host_suffixes.iter().any(|value| host.ends_with(value))
    }

    pub fn matches_extractor_key(&self, extractor_key: &str) -> bool {
        self.extractor_keywords
            .iter()
            .any(|value| extractor_key.contains(value))
    }
}

fn provider_catalog() -> &'static Vec<ProviderCatalogEntry> {
    static PROVIDER_CATALOG: OnceLock<Vec<ProviderCatalogEntry>> = OnceLock::new();

    PROVIDER_CATALOG.get_or_init(|| {
        serde_json::from_str(GENERATED_PROVIDER_CATALOG_JSON)
            .expect("failed to parse generated online-media provider catalog")
    })
}

pub fn find_provider_by_id(id: &str) -> Option<&'static ProviderCatalogEntry> {
    provider_catalog().iter().find(|entry| entry.id == id)
}

pub fn find_provider_by_host(host: &str) -> Option<&'static ProviderCatalogEntry> {
    provider_catalog()
        .iter()
        .find(|entry| entry.matches_host(host))
}

pub fn find_provider_by_extractor_key(
    extractor_key: &str,
) -> Option<&'static ProviderCatalogEntry> {
    provider_catalog()
        .iter()
        .find(|entry| entry.matches_extractor_key(extractor_key))
}

pub fn list_all_providers() -> Vec<ProviderListEntry> {
    provider_catalog()
        .iter()
        .map(|entry| ProviderListEntry {
            id: entry.id.clone(),
            name: entry.name.clone(),
            display_name: entry.display_name.clone(),
            source_site: entry.source_site.clone(),
            supported_content_types: entry.supported_content_types.clone(),
            requires_auth: entry.requires_auth,
            auth_configurable: entry.auth_configurable,
            common_source_sites: entry.common_source_sites.clone(),
            source_site_aliases: entry.source_site_aliases.clone(),
            host_suffixes: entry.host_suffixes.clone(),
        })
        .collect()
}

/// Merge native catalog entries with yt-dlp extractors.
/// The result is cached for the process lifetime.
pub async fn list_all_providers_with_ytdlp() -> ProviderListResponse {
    static CACHE: OnceCell<ProviderListResponse> = OnceCell::const_new();

    CACHE
        .get_or_init(|| async {
            let mut entries = list_all_providers();
            let ytdlp_available = crate::tooling::resolve_ytdlp_binary().is_some();

            if ytdlp_available {
                let ytdlp_entries = discover_ytdlp_extractors().await;
                info!(
                    native = entries.len(),
                    ytdlp = ytdlp_entries.len(),
                    "merged provider list"
                );
                entries.extend(ytdlp_entries);
            } else {
                warn!("yt-dlp binary not found; yt-dlp extractors unavailable");
            }

            ProviderListResponse {
                providers: entries,
                ytdlp_available,
            }
        })
        .await
        .clone()
}

/// Generic protocol keywords that should NOT be used for dedup filtering.
/// These appear as `extractorKeywords` for the "stream" meta-provider but
/// would incorrectly filter out unrelated yt-dlp extractors.
const GENERIC_PROTOCOL_KEYWORDS: &[&str] = &["m3u8", "dash", "stream", "hls"];

/// Known adult yt-dlp extractor names (lowercase).
/// Used to tag yt-dlp extractors with `"adult"` content type.
const KNOWN_ADULT_EXTRACTOR_KEYWORDS: &[&str] = &[
    "porn",
    "xxx",
    "xvideo",
    "xhamster",
    "xnxx",
    "tube8",
    "redtube",
    "youporn",
    "spankbang",
    "eporner",
    "hentai",
    "hanime",
    "rule34",
    "noodlemagazine",
    "txxx",
    "hclips",
    "erome",
    "cam4",
    "chaturbate",
    "bongacams",
    "stripchat",
    "4tube",
    "adult",
    "sexu",
    "beeg",
    "empflix",
    "motherless",
    "drtuber",
    "slutload",
    "fux",
    "hellporn",
    "alphaporn",
    "sunporno",
    "goshgay",
    "lovehomeporn",
    "nubilesporn",
    "pornbox",
    "pornerbros",
    "pornflip",
    "pornotube",
    "porntop",
    "porntube",
    "pornez",
    "porntrex",
    "tokyomotion",
    "iwara",
];

fn is_adult_extractor_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    KNOWN_ADULT_EXTRACTOR_KEYWORDS
        .iter()
        .any(|kw| lower.contains(kw))
}

/// Lazily-parsed yt-dlp extractor domain map (module name -> domain).
fn ytdlp_domain_map() -> &'static HashMap<String, String> {
    static MAP: OnceLock<HashMap<String, String>> = OnceLock::new();
    MAP.get_or_init(|| serde_json::from_str(YTDLP_DOMAINS_JSON).unwrap_or_else(|_| HashMap::new()))
}

/// Run `yt-dlp --list-extractors`, parse unique base extractor names,
/// and return them as `ProviderListEntry` items (excluding native providers).
async fn discover_ytdlp_extractors() -> Vec<ProviderListEntry> {
    let ytdlp_bin = if let Some(bin) = crate::tooling::resolve_ytdlp_binary() {
        info!(path = %bin.display(), "found yt-dlp binary");
        bin
    } else {
        warn!("yt-dlp binary not found; skipping extractor discovery");
        return Vec::new();
    };

    let output = match tokio::process::Command::new(&ytdlp_bin)
        .arg("--list-extractors")
        .output()
        .await
    {
        Ok(out) if out.status.success() => out,
        Ok(out) => {
            warn!(
                status = %out.status,
                "yt-dlp --list-extractors failed"
            );
            return Vec::new();
        }
        Err(err) => {
            warn!(%err, "failed to run yt-dlp --list-extractors");
            return Vec::new();
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Collect native provider IDs and names for exact-match dedup.
    let native_exact: HashSet<String> = provider_catalog()
        .iter()
        .flat_map(|e| vec![e.id.to_ascii_lowercase(), e.name.to_ascii_lowercase()])
        .collect();

    // Collect site-identifying keywords from native providers.
    // Exclude generic protocol keywords that would over-match unrelated extractors.
    let generic: HashSet<&str> = GENERIC_PROTOCOL_KEYWORDS.iter().copied().collect();
    let native_keywords: Vec<String> = provider_catalog()
        .iter()
        .flat_map(|e| {
            e.extractor_keywords
                .iter()
                .filter(|kw| !generic.contains(kw.as_str()))
                .cloned()
                .collect::<Vec<_>>()
        })
        .collect();

    // Parse extractor names: keep only base names (no `:` sub-extractors),
    // skip broken ones, deduplicate.
    let domain_map = ytdlp_domain_map();
    let mut seen = HashSet::new();
    let mut entries = Vec::new();

    for line in stdout.lines() {
        let name = line.trim();
        if name.is_empty() || name.contains("(CURRENTLY BROKEN)") {
            continue;
        }

        // Extract base name (before `:`) and use it as the dedup key.
        let base = if let Some(pos) = name.find(':') {
            &name[..pos]
        } else {
            name
        };
        let base_lower = base.to_ascii_lowercase();

        // Skip if we already have this base name.
        if !seen.insert(base_lower.clone()) {
            continue;
        }

        // Skip if it exactly matches a native provider ID or name.
        if native_exact.contains(&base_lower) {
            continue;
        }

        // Skip if the extractor name contains a native site keyword.
        // One-directional only: keyword must appear inside the extractor name.
        if native_keywords
            .iter()
            .any(|kw| base_lower.contains(kw.as_str()))
        {
            continue;
        }

        let is_adult = is_adult_extractor_name(base);
        let content_types = if is_adult {
            vec!["adult".to_string(), "online_video".to_string()]
        } else {
            vec!["online_video".to_string()]
        };

        entries.push(ProviderListEntry {
            id: format!("ytdlp:{base_lower}"),
            name: base.to_string(),
            display_name: base.to_string(),
            source_site: base.to_string(),
            supported_content_types: content_types,
            requires_auth: false,
            auth_configurable: false,
            common_source_sites: vec![base.to_string()],
            source_site_aliases: vec![],
            host_suffixes: domain_map
                .get(&base_lower)
                .map(|d| vec![d.clone()])
                .unwrap_or_default(),
        });
    }

    debug!(
        native_count = provider_catalog().len(),
        ytdlp_count = entries.len(),
        "discovered yt-dlp extractors"
    );

    entries
}

pub fn is_likely_adult_source_site(source_site: &str) -> bool {
    let normalized = source_site.to_ascii_lowercase();
    [
        "porn",
        "xvideo",
        "xhamster",
        "iwara",
        "hanime",
        "tokyomotion",
        "xnxx",
        "youporn",
        "fc2",
    ]
    .iter()
    .any(|value| normalized.contains(value))
}
