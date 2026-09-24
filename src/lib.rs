#![forbid(unsafe_code)]

//! The TOML content contract — a technology of `xmip-core-contract`.
//!
//! Two claims (ADR-0042): **well-formedness is a given** — every Stream is
//! parsed as TOML and a failure names its line and column — and **conformance
//! is a given once the contract is named** — a Receive or Send Location that
//! refers to this contract with a layout bound has every Stream held to the
//! keys and types the layout names. A bare contract is the first claim alone.
//!
//! The layout language is the small TOML document [`layout`] documents: a
//! required key per leaf, its type as the value, nested by dotted path.

pub mod layout;

use layout::Layout;
use sdk::contract::{
    Contract, ContractDescriptor, ContractError, ContractFactory, ContractId, ValidationIssue,
    ValidationResult,
};
use stream::Stream;
use toml::Table;

const REPRESENTATION: &str = "application/toml";

/// The TOML contract, bare or bound to a layout.
pub struct Toml {
    descriptor: ContractDescriptor,
    layout: Option<Layout>,
}

impl Toml {
    /// Well-formedness only.
    #[must_use]
    pub fn new() -> Self {
        Self {
            descriptor: descriptor("toml"),
            layout: None,
        }
    }

    /// Well-formedness and conformance to `layout`, named `name` in the
    /// descriptor.
    #[must_use]
    pub fn with_layout(name: &str, layout: Layout) -> Self {
        Self {
            descriptor: descriptor(&format!("toml:{name}")),
            layout: Some(layout),
        }
    }

    /// Whether a layout is bound.
    #[must_use]
    pub const fn is_bound(&self) -> bool {
        self.layout.is_some()
    }
}

impl Default for Toml {
    fn default() -> Self {
        Self::new()
    }
}

fn descriptor(id: &str) -> ContractDescriptor {
    ContractDescriptor {
        id: ContractId(id.to_string()),
        version: "1".to_string(),
        representation: REPRESENTATION.to_string(),
    }
}

impl Contract for Toml {
    fn descriptor(&self) -> &ContractDescriptor {
        &self.descriptor
    }

    fn identify(&self, stream: &Stream) -> Result<bool, ContractError> {
        if let Some(media_type) = stream.media_type() {
            return Ok(is_toml_media_type(media_type));
        }
        Ok(std::str::from_utf8(stream.bytes()).is_ok_and(looks_like_toml))
    }

    fn validate(&self, stream: &Stream) -> Result<ValidationResult, ContractError> {
        let text = std::str::from_utf8(stream.bytes()).map_err(|error| ContractError {
            message: format!("not UTF-8 text: {error}"),
        })?;
        let document = match text.parse::<Table>() {
            Ok(document) => document,
            Err(error) => return Ok(malformed(text, &error)),
        };
        let issues = self
            .layout
            .as_ref()
            .map_or_else(Vec::new, |layout| layout.check(&document));
        Ok(ValidationResult::of(issues))
    }
}

fn is_toml_media_type(media_type: &str) -> bool {
    let essence = media_type.split(';').next().unwrap_or("").trim();
    essence.eq_ignore_ascii_case("application/toml") || essence.ends_with("+toml")
}

/// The first line that is neither blank nor a comment is a table header or a
/// `key = value` with a TOML-shaped key.
fn looks_like_toml(text: &str) -> bool {
    let Some(line) = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
    else {
        return false;
    };
    if line.starts_with('[') {
        return line.ends_with(']');
    }
    line.split_once('=').is_some_and(|(key, _)| {
        let key = key.trim();
        !key.is_empty()
            && key.chars().all(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '"' | '\'' | ' ')
            })
    })
}

fn malformed(text: &str, error: &toml::de::Error) -> ValidationResult {
    let path = error.span().map(|span| {
        let before = &text[..span.start.min(text.len())];
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
        format!("line {line} column {column}")
    });
    ValidationResult::of(vec![ValidationIssue::new(
        "malformed",
        &format!("not valid TOML: {}", error.message()),
        path,
    )])
}

/// Loads the contract a Location names: `toml` or an empty reference is the
/// bare contract, anything else is the path of a layout file.
pub struct TomlFactory;

impl ContractFactory for TomlFactory {
    fn technology(&self) -> &'static str {
        "toml"
    }

    fn load(&self, reference: &str) -> Result<Box<dyn Contract>, ContractError> {
        let reference = reference.trim();
        if reference.is_empty() || reference == self.technology() {
            return Ok(Box::new(Toml::new()));
        }
        let text = std::fs::read_to_string(reference).map_err(|error| ContractError {
            message: format!("cannot read layout {reference}: {error}"),
        })?;
        let layout = Layout::parse(&text).map_err(|error| ContractError {
            message: format!("layout {reference}: {error}"),
        })?;
        let name = std::path::Path::new(reference)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(reference);
        Ok(Box::new(Toml::with_layout(name, layout)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use contract::fixture::stream_as as stream;
    use xcore::StreamId;

    fn service_layout() -> Layout {
        Layout::parse("service.name = \"string\"\nservice.port = \"integer\"").expect("a layout")
    }

    #[test]
    fn the_bare_contract_holds_well_formed_toml_and_names_where_it_breaks() {
        let bare = Toml::new();
        assert!(!bare.is_bound());
        assert_eq!(bare.descriptor().id.0, "toml");
        let held = bare
            .validate(&stream("[any]\nshape = true", None))
            .expect("validates");
        assert!(held.valid, "{:?}", held.issues);
        let broken = bare
            .validate(&stream("[service]\nname = \"a\"\nport = \n", None))
            .expect("validates");
        assert!(!broken.valid);
        assert_eq!(broken.issues[0].code, "malformed");
        assert!(
            broken.issues[0]
                .path
                .as_deref()
                .is_some_and(|path| path.starts_with("line 3 column ")),
            "{:?}",
            broken.issues[0].path
        );
        let bytes = Stream::new(StreamId::new(2), vec![0xff, 0xfe], None);
        assert!(
            bare.validate(&bytes).is_err(),
            "not text is an error, not an issue"
        );
    }

    #[test]
    fn the_bound_contract_holds_the_layout_and_names_every_departure() {
        let bound = Toml::with_layout("service", service_layout());
        assert!(bound.is_bound());
        assert_eq!(bound.descriptor().id.0, "toml:service");
        let held = bound
            .validate(&stream(
                "[service]\nname = \"edge\"\nport = 80\nextra = 1",
                None,
            ))
            .expect("validates");
        assert!(held.valid, "{:?}", held.issues);
        let departed = bound
            .validate(&stream("[service]\nname = 1", None))
            .expect("validates");
        assert!(!departed.valid);
        let codes: Vec<(&str, Option<&str>)> = departed
            .issues
            .iter()
            .map(|issue| (issue.code.as_str(), issue.path.as_deref()))
            .collect();
        assert_eq!(
            codes,
            vec![
                ("type", Some("service.name")),
                ("required", Some("service.port"))
            ]
        );
    }

    #[test]
    fn identifies_by_media_type_or_by_the_first_line() {
        let bare = Toml::new();
        let is = |text: &str, media: Option<&str>| bare.identify(&stream(text, media)).expect("ok");
        assert!(is("x", Some("application/toml")));
        assert!(is("x", Some("application/toml; charset=utf-8")));
        assert!(!is("[a]", Some("text/csv")));
        assert!(is("# comment\n\n[service]\nname = \"a\"", None));
        assert!(is("name = \"a\"", None));
        assert!(is("service.name = \"a\"", None));
        assert!(!is("name: a", None));
        assert!(!is("{\"name\": \"a\"}", None));
        assert!(!is("", None));
    }

    #[test]
    fn the_factory_loads_bare_and_bound_and_refuses_a_bad_layout() {
        let factory = TomlFactory;
        assert_eq!(factory.technology(), "toml");
        assert_eq!(factory.load("").expect("bare").descriptor().id.0, "toml");
        assert_eq!(
            factory.load("toml").expect("bare").descriptor().id.0,
            "toml"
        );
        let dir = std::env::temp_dir().join("xmip-contract-toml-test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("service.layout.toml");
        std::fs::write(&file, "service.name = \"string\"").expect("write layout");
        let bound = factory.load(file.to_str().expect("path")).expect("bound");
        assert_eq!(bound.descriptor().id.0, "toml:service.layout");
        let bad = dir.join("bad.layout.toml");
        std::fs::write(&bad, "service.name = \"text\"").expect("write layout");
        let refused = factory
            .load(bad.to_str().expect("path"))
            .err()
            .expect("refused");
        assert!(refused.message.contains("service.name asks for \"text\""));
        assert!(
            factory
                .load(dir.join("missing.toml").to_str().expect("path"))
                .is_err()
        );
    }

    #[test]
    fn the_edge_node_sample_is_well_formed_and_holds_its_service_layout() {
        let sample = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../operation/gui/samples/edge-01.xmip.toml");
        let Ok(text) = std::fs::read_to_string(&sample) else {
            eprintln!("sample {} is absent; skipped", sample.display());
            return;
        };
        let bare = Toml::new();
        let held = bare
            .validate(&stream(&text, Some("application/toml")))
            .expect("validates");
        assert!(held.valid, "{:?}", held.issues);
        let layout = Layout::parse(
            "service.name = \"string\"\nservice.cluster_name = \"string\"\n\
             modules = \"array\"\nreceive_locations = \"array\"",
        )
        .expect("a layout");
        let bound = Toml::with_layout("edge", layout);
        let held = bound.validate(&stream(&text, None)).expect("validates");
        assert!(held.valid, "{:?}", held.issues);
    }
}
