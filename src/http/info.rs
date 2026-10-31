//! Cluster flavor/version detection from `GET /`.

use semver::Version;
use sonic_rs::{JsonContainerTrait, JsonValueTrait};

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

/// The version OpenSearch 1.x reports on `GET /` with
/// `compatibility.override_main_response_version: true`.
const OVERRIDE_VERSION: &str = "7.10.2";

/// `filter_path` of the `GET _nodes` request of [`Probe::resolve`].
pub const NODES_FILTER: &str = "nodes.*.version,nodes.*.build_flavor";

/// What `GET /` alone says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detection {
    Known(ClusterInfo),
    /// `GET /` reports `7.10.2` without `version.distribution` and either has the
    /// OpenSearch tagline or no `build_flavor`: OpenSearch with
    /// `compatibility.override_main_response_version`, or something rewriting `GET /`.
    Probe(Probe),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    number: String,
    tagline_os: bool,
}

impl Probe {
    /// Decide from `GET _nodes` (`nodes`: its 2xx body, or why there is none).
    /// OpenSearch nodes report their real version there and no `build_flavor`;
    /// Elasticsearch nodes always have a `build_flavor`.
    pub fn resolve(&self, nodes: Option<Result<&[u8], String>>) -> anyhow::Result<ClusterInfo> {
        let number = &self.number;
        let mut why = match &nodes {
            None => "not probed".to_string(),
            Some(Err(e)) => e.clone(),
            Some(Ok(_)) => String::new(),
        };
        if let Some(Ok(body)) = nodes {
            match nodes_versions(body) {
                Ok((versions, true)) => {
                    let version = versions
                        .into_iter()
                        .min()
                        .unwrap_or_else(|| Version::new(7, 10, 2));
                    return Ok(ClusterInfo {
                        flavor: Flavor::Elastic,
                        version,
                        oss: false,
                    });
                }
                Ok((versions, _)) => match versions.into_iter().min() {
                    Some(v) if v.major < 7 => {
                        log::warn!(
                            "OpenSearch {v} reports version {number} on GET / (compatibility.override_main_response_version is on); using {v} from GET _nodes"
                        );
                        return Ok(ClusterInfo {
                            flavor: Flavor::OpenSearch,
                            version: v,
                            oss: false,
                        });
                    }
                    Some(v) => why = format!("GET _nodes reports version {v}"),
                    None => why = "GET _nodes listed no node versions".into(),
                },
                Err(e) => why = e,
            }
        }
        if self.tagline_os {
            log::warn!(
                "OpenSearch reports version {number} without a distribution (compatibility.override_main_response_version is on) and GET _nodes could not tell its real version ({why}); assuming the OpenSearch 1.x feature set"
            );
            return Ok(ClusterInfo {
                flavor: Flavor::OpenSearch,
                version: Version::new(1, 0, 0),
                oss: false,
            });
        }
        anyhow::bail!(
            "GET / reports version {number} with neither version.distribution nor version.build_flavor (Elasticsearch 7.10.2 always sends build_flavor): this looks like OpenSearch with compatibility.override_main_response_version=true, or a proxy rewriting GET /, and GET _nodes could not tell ({why}); set compatibility.override_main_response_version to false on the cluster or connect without the proxy"
        )
    }
}

/// Node versions in a `GET _nodes` body, and whether any node has a `build_flavor`.
fn nodes_versions(body: &[u8]) -> Result<(Vec<Version>, bool), String> {
    let v: sonic_rs::Value = sonic_rs::from_slice(body)
        .map_err(|_| format!("GET _nodes did not return JSON: {}", snippet(body)))?;
    let Some(nodes) = v["nodes"].as_object() else {
        return Err(format!("GET _nodes returned no nodes: {}", snippet(body)));
    };
    let mut versions = Vec::new();
    let mut elastic = false;
    for (_, n) in nodes.iter() {
        elastic |= n["build_flavor"].as_str().is_some();
        let raw = n["version"].as_str().unwrap_or_default();
        versions.push(
            lenient_version(raw)
                .ok_or_else(|| format!("GET _nodes: unparsable version {raw:?}"))?,
        );
    }
    Ok((versions, elastic))
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

    /// Parse a `GET /` response, without a follow-up probe: an OpenSearch in
    /// override mode (see [`Detection::Probe`]) is resolved from `GET /` alone.
    pub fn parse(body: &[u8]) -> anyhow::Result<Self> {
        match Self::detect(body)? {
            Detection::Known(i) => Ok(i),
            Detection::Probe(p) => p.resolve(None),
        }
    }

    /// Read a `GET /` response.
    ///
    /// Unlike elasticdump, nothing falls back silently to "Elasticsearch
    /// 5.0.0": a body that is not JSON (a proxy or login page) or has no
    /// `version.number` is an error naming what came back. OpenSearch is
    /// recognised by `version.distribution` or, when that is missing, by the
    /// tagline. OpenSearch 1.x with `compatibility.override_main_response_version`
    /// reports Elasticsearch 7.10.2 and no distribution (live, L-654: the tagline
    /// stays OpenSearch's): that, and a 7.10.2 with neither `distribution` nor
    /// `build_flavor` (which Elasticsearch >= 6.3 always sends), needs the `GET _nodes`
    /// probe of [`Probe::resolve`].
    pub fn detect(body: &[u8]) -> anyhow::Result<Detection> {
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
        let build_flavor = v["version"]["build_flavor"].as_str();
        let tagline_os = v["tagline"]
            .as_str()
            .is_some_and(|t| t.contains("OpenSearch"));
        if distribution != Some("opensearch")
            && (tagline_os || (build_flavor.is_none() && number == OVERRIDE_VERSION))
        {
            return Ok(Detection::Probe(Probe {
                number: number.to_string(),
                tagline_os,
            }));
        }
        let flavor = if distribution == Some("opensearch") {
            Flavor::OpenSearch
        } else {
            Flavor::Elastic
        };
        Ok(Detection::Known(Self {
            flavor,
            version: parsed,
            oss: flavor == Flavor::Elastic && build_flavor == Some("oss"),
        }))
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

    /// The ISM `shrink` action (OpenSearch >= 2.0; 1.3 answers "Invalid field:
    /// [shrink] found in Actions", found by the L-507 oracle).
    pub fn has_ism_shrink(&self) -> bool {
        self.flavor == Flavor::OpenSearch && self.version >= Version::new(2, 0, 0)
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

    // L-654 (BRK-XV-001): the live OpenSearch 1.3.20 `GET /` with
    // `compatibility.override_main_response_version: true`, and what a proxy rewriting
    // the tagline makes of it.
    const OVERRIDE_ROOT: &str = r#"{"name":"n","version":{"number":"7.10.2","build_type":"tar","lucene_version":"8.10.1"},"tagline":"The OpenSearch Project: https://opensearch.org/"}"#;
    const REWRITTEN_ROOT: &str = r#"{"name":"n","version":{"number":"7.10.2","build_type":"tar"},"tagline":"You Know, for Search"}"#;

    fn probe(root: &str) -> Probe {
        match ClusterInfo::detect(root.as_bytes()).unwrap() {
            Detection::Probe(p) => p,
            Detection::Known(i) => panic!("{root} needs a probe, got {i:?}"),
        }
    }

    #[test]
    fn override_mode_takes_the_real_version_from_the_nodes() {
        for root in [OVERRIDE_ROOT, REWRITTEN_ROOT] {
            let nodes = br#"{"nodes":{"a":{"version":"1.3.20"},"b":{"version":"1.3.19"}}}"#;
            let i = probe(root).resolve(Some(Ok(nodes))).unwrap();
            assert!(i.is_opensearch(), "{root}");
            assert_eq!(i.version, Version::new(1, 3, 19), "oldest node, {root}");
            assert!(!i.oss);
            assert_eq!(i.pit_api(), None);
        }
    }

    #[test]
    fn override_mode_without_an_answer_from_the_nodes() {
        // The OpenSearch tagline still says OpenSearch: the 1.x feature set is assumed.
        for nodes in [
            None,
            Some(Err("HTTP 403 Forbidden".to_string())),
            Some(Ok(&b"<html>login</html>"[..])),
            Some(Ok(&br#"{"nodes":{}}"#[..])),
            Some(Ok(&br#"{"nodes":{"a":{"version":"7.10.2"}}}"#[..])),
        ] {
            let i = probe(OVERRIDE_ROOT).resolve(nodes.clone()).unwrap();
            assert!(i.is_opensearch(), "{nodes:?}");
            assert_eq!(i.version, Version::new(1, 0, 0));
            // Nothing says OpenSearch: fail clearly rather than guess Elasticsearch.
            let e = format!(
                "{:#}",
                probe(REWRITTEN_ROOT).resolve(nodes.clone()).unwrap_err()
            );
            assert!(
                e.contains("compatibility.override_main_response_version")
                    && e.contains("neither version.distribution nor version.build_flavor"),
                "{e}"
            );
        }
        let e = format!(
            "{:#}",
            probe(REWRITTEN_ROOT)
                .resolve(Some(Err("HTTP 403 Forbidden".into())))
                .unwrap_err()
        );
        assert!(e.contains("HTTP 403 Forbidden"), "{e}");
        assert!(ClusterInfo::parse(REWRITTEN_ROOT.as_bytes()).is_err());
    }

    #[test]
    fn elasticsearch_nodes_win_over_an_ambiguous_root() {
        let nodes = br#"{"nodes":{"a":{"version":"7.10.2","build_flavor":"default"}}}"#;
        for root in [OVERRIDE_ROOT, REWRITTEN_ROOT] {
            let i = probe(root).resolve(Some(Ok(nodes))).unwrap();
            assert_eq!(i.flavor, Flavor::Elastic, "{root}");
            assert_eq!(i.version, Version::new(7, 10, 2));
        }
    }

    #[test]
    fn only_ambiguous_roots_are_probed() {
        for root in [
            r#"{"version":{"number":"7.10.2","build_flavor":"default"},"tagline":"You Know, for Search"}"#,
            r#"{"version":{"number":"7.10.2","build_flavor":"oss"}}"#,
            r#"{"version":{"distribution":"opensearch","number":"1.3.20"},"tagline":"The OpenSearch Project: https://opensearch.org/"}"#,
            r#"{"version":{"number":"7.17.29"}}"#,
            r#"{"version":{"number":"6.2.4"},"tagline":"You Know, for Search"}"#,
        ] {
            assert!(
                matches!(
                    ClusterInfo::detect(root.as_bytes()).unwrap(),
                    Detection::Known(_)
                ),
                "{root}"
            );
        }
    }

    impl ClusterInfo {
        fn parse_ok(b: &[u8]) -> Self {
            Self::parse(b).unwrap()
        }
    }
}
