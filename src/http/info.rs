//! Cluster flavor/version detection from `GET /`.

use semver::Version;
use sonic_rs::JsonValueTrait;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Elastic,
    OpenSearch,
}

/// Which point-in-time API the cluster speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PitApi {
    /// `POST /{index}/_pit`, `DELETE /_pit {"id"}` (Elasticsearch >= 7.10).
    Elastic,
    /// `POST /{index}/_search/point_in_time`, `DELETE /_search/point_in_time
    /// {"pit_id":[...]}` (OpenSearch >= 2.4).
    OpenSearch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterInfo {
    pub flavor: Flavor,
    pub version: Version,
    /// Elasticsearch OSS build (`version.build_flavor == "oss"`, 6.3-7.10):
    /// no X-Pack, so no PIT, ILM or security.
    pub oss: bool,
}

fn snippet(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = flat.chars().take(120).collect();
    if flat.chars().count() > 120 {
        out.push('…');
    }
    if out.is_empty() {
        "(empty body)".into()
    } else {
        out
    }
}

fn lenient_version(raw: &str) -> Option<Version> {
    if let Ok(v) = Version::parse(raw) {
        return Some(v);
    }
    let core: Vec<u64> = raw
        .split(['-', '+'])
        .next()?
        .split('.')
        .map(|p| p.parse().ok())
        .collect::<Option<Vec<_>>>()?;
    Some(Version::new(
        *core.first()?,
        core.get(1).copied().unwrap_or(0),
        core.get(2).copied().unwrap_or(0),
    ))
}

impl ClusterInfo {
    pub fn new(flavor: Flavor, version: &str) -> Self {
        Self {
            flavor,
            version: lenient_version(version).unwrap_or(Version::new(5, 0, 0)),
            oss: false,
        }
    }

    /// Parse a `GET /` response.
    ///
    /// Unlike elasticdump, nothing falls back silently to "Elasticsearch
    /// 5.0.0": a body that is not JSON (a proxy or login page) or has no
    /// `version.number` is an error naming what came back. OpenSearch is
    /// recognised by `version.distribution` or, when that is missing, by the
    /// tagline. OpenSearch 1.x with `compatibility.override_main_response_version`
    /// reports Elasticsearch 7.10.2 and no distribution; that is treated as
    /// OpenSearch 1.0 (the most conservative feature set) with a warning.
    pub fn parse(body: &[u8]) -> anyhow::Result<Self> {
        let Ok(v) = sonic_rs::from_slice::<sonic_rs::Value>(body) else {
            anyhow::bail!(
                "GET / did not return JSON, so this does not look like an Elasticsearch/OpenSearch endpoint (a proxy or login page?); body starts with: {}",
                snippet(body)
            );
        };
        let Some(number) = v["version"]["number"].as_str() else {
            anyhow::bail!(
                "GET / returned JSON without version.number, so the engine and version cannot be detected; body starts with: {}",
                snippet(body)
            );
        };
        let Some(parsed) = lenient_version(number) else {
            anyhow::bail!("GET / reported an unparsable version.number {number:?}");
        };
        let distribution = v["version"]["distribution"].as_str();
        let tagline_os = v["tagline"]
            .as_str()
            .is_some_and(|t| t.contains("OpenSearch"));
        let flavor = if distribution == Some("opensearch") || tagline_os {
            Flavor::OpenSearch
        } else {
            Flavor::Elastic
        };
        let mut info = Self {
            flavor,
            version: parsed,
            oss: false,
        };
        if flavor == Flavor::OpenSearch && distribution != Some("opensearch") {
            // `compatibility.override_main_response_version` (OpenSearch 1.x).
            log::warn!(
                "OpenSearch reports version {number} without a distribution (compatibility.override_main_response_version is on); assuming the OpenSearch 1.x feature set"
            );
            info.version = Version::new(1, 0, 0);
        }
        info.oss =
            flavor == Flavor::Elastic && v["version"]["build_flavor"].as_str() == Some("oss");
        Ok(info)
    }

    pub fn flavor_name(&self) -> &'static str {
        match self.flavor {
            Flavor::Elastic => "Elasticsearch",
            Flavor::OpenSearch => "OpenSearch",
        }
    }

    pub fn is_opensearch(&self) -> bool {
        self.flavor == Flavor::OpenSearch
    }

    /// Mapping types / `_type` in bulk actions (Elasticsearch < 7).
    pub fn has_types(&self) -> bool {
        self.flavor == Flavor::Elastic && self.version.major < 7
    }

    /// `_timestamp` / `_ttl` bulk metadata (Elasticsearch < 5 only).
    pub fn supports_ttl_meta(&self) -> bool {
        self.flavor == Flavor::Elastic && self.version.major < 5
    }

    pub fn pit_api(&self) -> Option<PitApi> {
        match self.flavor {
            Flavor::Elastic if self.version >= Version::new(7, 10, 0) && !self.oss => {
                Some(PitApi::Elastic)
            }
            Flavor::OpenSearch if self.version >= Version::new(2, 4, 0) => Some(PitApi::OpenSearch),
            _ => None,
        }
    }

    /// `slice` inside a PIT search: Elasticsearch >= 7.15 (older versions
    /// answer "`slice` cannot be used outside of a scroll context").
    pub fn supports_sliced_pit(&self) -> bool {
        match self.flavor {
            Flavor::Elastic => self.version >= Version::new(7, 15, 0),
            Flavor::OpenSearch => true,
        }
    }

    /// `_shard_doc` sort tiebreaker: Elasticsearch >= 7.12 only. OpenSearch
    /// 2.19 rejects it (`No mapping found for [_shard_doc]`).
    pub fn supports_shard_doc_sort(&self) -> bool {
        self.flavor == Flavor::Elastic && self.version >= Version::new(7, 12, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_opensearch() {
        let i = ClusterInfo::parse_ok(
            br#"{"version":{"distribution":"opensearch","number":"2.19.7"}}"#,
        );
        assert!(i.is_opensearch());
        assert_eq!(i.pit_api(), Some(PitApi::OpenSearch));
        assert!(!i.supports_shard_doc_sort());
        assert!(!i.has_types());
    }

    #[test]
    fn detects_elasticsearch_versions() {
        let i = ClusterInfo::parse_ok(br#"{"version":{"number":"8.15.0"}}"#);
        assert_eq!(i.flavor, Flavor::Elastic);
        assert_eq!(i.pit_api(), Some(PitApi::Elastic));
        assert!(i.supports_shard_doc_sort());
        let old = ClusterInfo::parse_ok(br#"{"version":{"number":"6.8.23"}}"#);
        assert!(old.has_types());
        assert_eq!(old.pit_api(), None);
        let oss = ClusterInfo::parse_ok(br#"{"version":{"number":"7.10.2","build_flavor":"oss"}}"#);
        assert!(oss.oss);
        assert_eq!(oss.pit_api(), None);
        let default =
            ClusterInfo::parse_ok(br#"{"version":{"number":"7.10.2","build_flavor":"default"}}"#);
        assert_eq!(default.pit_api(), Some(PitApi::Elastic));
        assert!(!default.supports_sliced_pit());
        assert!(!default.supports_shard_doc_sort());
        let snap = ClusterInfo::parse_ok(br#"{"version":{"number":"7.10.0-SNAPSHOT"}}"#);
        assert_eq!(snap.version.major, 7);
    }

    #[test]
    fn never_falls_back_silently() {
        let e = ClusterInfo::parse(b"<html><body>Sign in</body></html>").unwrap_err();
        assert!(e.to_string().contains("did not return JSON"), "{e}");
        assert!(e.to_string().contains("<html><body>Sign in"), "{e}");
        let e = ClusterInfo::parse(b"").unwrap_err();
        assert!(e.to_string().contains("(empty body)"), "{e}");
        let e = ClusterInfo::parse(br#"{"name":"x","cluster_name":"y"}"#).unwrap_err();
        assert!(e.to_string().contains("without version.number"), "{e}");
        let e = ClusterInfo::parse(br#"{"version":{"number":"banana"}}"#).unwrap_err();
        assert!(e.to_string().contains("unparsable"), "{e}");
    }

    #[test]
    fn opensearch_by_tagline_and_override_mode() {
        let tag = r#""tagline":"The OpenSearch Project: https://opensearch.org/""#;
        let i = ClusterInfo::parse_ok(
            format!(r#"{{"version":{{"distribution":"opensearch","number":"1.3.20"}},{tag}}}"#)
                .as_bytes(),
        );
        assert!(i.is_opensearch());
        assert_eq!(i.version, Version::new(1, 3, 20));
        // compatibility.override_main_response_version=true (OpenSearch 1.x)
        let o = ClusterInfo::parse_ok(
            format!(r#"{{"version":{{"number":"7.10.2","build_flavor":"oss"}},{tag}}}"#).as_bytes(),
        );
        assert!(o.is_opensearch());
        assert!(!o.oss);
        assert_eq!(o.version, Version::new(1, 0, 0));
        assert_eq!(o.pit_api(), None);
        assert!(!o.has_types());
        let es = ClusterInfo::parse_ok(
            br#"{"version":{"number":"7.10.2","build_flavor":"oss"},"tagline":"You Know, for Search"}"#,
        );
        assert_eq!(es.flavor, Flavor::Elastic);
        assert!(es.oss);
    }

    impl ClusterInfo {
        fn parse_ok(b: &[u8]) -> Self {
            Self::parse(b).unwrap()
        }
    }
}
