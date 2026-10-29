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
        }
    }

    /// Parse a `GET /` response. Missing or unparsable version information
    /// falls back to Elasticsearch 5.0.0, like elasticdump.
    pub fn parse(body: &[u8]) -> Self {
        let parsed: Option<sonic_rs::Value> = sonic_rs::from_slice(body).ok();
        let Some(v) = parsed else {
            return Self::new(Flavor::Elastic, "5.0.0");
        };
        let number = v["version"]["number"].as_str().unwrap_or("5.0.0");
        let flavor = if v["version"]["distribution"].as_str() == Some("opensearch") {
            Flavor::OpenSearch
        } else {
            Flavor::Elastic
        };
        Self::new(flavor, number)
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
            Flavor::Elastic if self.version >= Version::new(7, 10, 0) => Some(PitApi::Elastic),
            Flavor::OpenSearch if self.version >= Version::new(2, 4, 0) => Some(PitApi::OpenSearch),
            _ => None,
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
        let i =
            ClusterInfo::parse(br#"{"version":{"distribution":"opensearch","number":"2.19.7"}}"#);
        assert!(i.is_opensearch());
        assert_eq!(i.pit_api(), Some(PitApi::OpenSearch));
        assert!(!i.supports_shard_doc_sort());
        assert!(!i.has_types());
    }

    #[test]
    fn detects_elasticsearch_versions() {
        let i = ClusterInfo::parse(br#"{"version":{"number":"8.15.0"}}"#);
        assert_eq!(i.flavor, Flavor::Elastic);
        assert_eq!(i.pit_api(), Some(PitApi::Elastic));
        assert!(i.supports_shard_doc_sort());
        let old = ClusterInfo::parse(br#"{"version":{"number":"6.8.23"}}"#);
        assert!(old.has_types());
        assert_eq!(old.pit_api(), None);
        let snap = ClusterInfo::parse(br#"{"version":{"number":"7.10.0-SNAPSHOT"}}"#);
        assert_eq!(snap.version.major, 7);
    }

    #[test]
    fn falls_back_to_5_0_0() {
        let i = ClusterInfo::parse(b"not json");
        assert_eq!(i.version, Version::new(5, 0, 0));
        assert!(i.has_types());
    }
}
