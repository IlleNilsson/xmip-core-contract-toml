//! The layout a Location binds to the TOML contract: which keys a document
//! must hold, and of what type each.
//!
//! A layout is itself a small TOML document. Every leaf is a key the document
//! must have, and its value names the type: `string`, `integer`, `float`,
//! `boolean`, `table`, `array` or `datetime`. Keys nest by dotted path, so
//! `service.name = "string"` and a `[service]` table with `name = "string"`
//! say the same thing. Anything a layout does not name is not the layout's
//! business: a document may carry more than it asks for.
//!
//! A type the layout does not know is refused when it is bound, not when a
//! Stream arrives (ADR-0042), and an issue names the dotted path where the
//! document departed.

use contract::{ContractError, ValidationIssue};
use toml::{Table, Value};

/// The seven types a layout may ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    String,
    Integer,
    Float,
    Boolean,
    Table,
    Array,
    Datetime,
}

impl Kind {
    /// The type named in a layout, if the name is one of the seven.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "string" => Self::String,
            "integer" => Self::Integer,
            "float" => Self::Float,
            "boolean" => Self::Boolean,
            "table" => Self::Table,
            "array" => Self::Array,
            "datetime" => Self::Datetime,
            _ => return None,
        })
    }

    /// Whether `value` is of this kind.
    #[must_use]
    pub const fn holds(self, value: &Value) -> bool {
        matches!(
            (self, value),
            (Self::String, Value::String(_))
                | (Self::Integer, Value::Integer(_))
                | (Self::Float, Value::Float(_))
                | (Self::Boolean, Value::Boolean(_))
                | (Self::Table, Value::Table(_))
                | (Self::Array, Value::Array(_))
                | (Self::Datetime, Value::Datetime(_))
        )
    }

    /// The name a layout would use for `value`.
    #[must_use]
    pub const fn of(value: &Value) -> &'static str {
        match value {
            Value::String(_) => "string",
            Value::Integer(_) => "integer",
            Value::Float(_) => "float",
            Value::Boolean(_) => "boolean",
            Value::Table(_) => "table",
            Value::Array(_) => "array",
            Value::Datetime(_) => "datetime",
        }
    }
}

/// One key the layout requires: its dotted path and its type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Required {
    pub path: String,
    pub kind: Kind,
}

/// The keys a bound layout requires, in the order the layout names them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    required: Vec<Required>,
}

impl Layout {
    /// Read a layout document.
    ///
    /// # Errors
    /// The layout is not TOML, a leaf is not a type name, or a name is not
    /// one of the seven types.
    pub fn parse(text: &str) -> Result<Self, ContractError> {
        let table: Table = text
            .parse()
            .map_err(|error: toml::de::Error| ContractError {
                message: format!("the layout is not TOML: {}", error.message()),
            })?;
        let mut required = Vec::new();
        collect(&table, "", &mut required)?;
        Ok(Self { required })
    }

    /// The keys required, in layout order.
    #[must_use]
    pub fn required(&self) -> &[Required] {
        &self.required
    }

    /// Every way `document` departs from the layout: a required key that is
    /// missing, or one of another type than the layout names.
    #[must_use]
    pub fn check(&self, document: &Table) -> Vec<ValidationIssue> {
        self.required
            .iter()
            .filter_map(|required| match lookup(document, &required.path) {
                None => Some(ValidationIssue {
                    code: "required".to_string(),
                    message: format!("{} is required", required.path),
                    path: Some(required.path.clone()),
                }),
                Some(value) if !required.kind.holds(value) => Some(ValidationIssue {
                    code: "type".to_string(),
                    message: format!(
                        "{} is {}, the layout asks for {:?}",
                        required.path,
                        Kind::of(value),
                        required.kind
                    )
                    .to_lowercase(),
                    path: Some(required.path.clone()),
                }),
                Some(_) => None,
            })
            .collect()
    }
}

fn collect(table: &Table, prefix: &str, into: &mut Vec<Required>) -> Result<(), ContractError> {
    for (key, value) in table {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            Value::Table(nested) => collect(nested, &path, into)?,
            Value::String(name) => match Kind::named(name) {
                Some(kind) => into.push(Required { path, kind }),
                None => {
                    return Err(ContractError {
                        message: format!(
                            "{path} asks for {name:?}; a layout type is string, integer, \
                             float, boolean, table, array or datetime"
                        ),
                    });
                }
            },
            other => {
                return Err(ContractError {
                    message: format!(
                        "{path} is {}; a layout leaf names a type as a string",
                        Kind::of(other)
                    ),
                });
            }
        }
    }
    Ok(())
}

fn lookup<'a>(document: &'a Table, path: &str) -> Option<&'a Value> {
    let mut current: Option<&Value> = None;
    let mut table = document;
    for segment in path.split('.') {
        let value = table.get(segment)?;
        current = Some(value);
        table = match value {
            Value::Table(nested) => nested,
            _ => &EMPTY,
        };
    }
    current
}

static EMPTY: std::sync::LazyLock<Table> = std::sync::LazyLock::new(Table::new);

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(layout: &Layout) -> Vec<&str> {
        let mut paths: Vec<&str> = layout.required().iter().map(|r| r.path.as_str()).collect();
        paths.sort_unstable();
        paths
    }

    #[test]
    fn a_layout_lists_its_keys_by_dotted_path_however_they_are_written() {
        let dotted = Layout::parse(
            "service.name = \"string\"
modules = \"array\"",
        )
        .expect("a layout");
        let sectioned = Layout::parse(
            "modules = \"array\"
[service]
name = \"string\"",
        )
        .expect("a layout");
        assert_eq!(paths(&dotted), vec!["modules", "service.name"]);
        assert_eq!(paths(&dotted), paths(&sectioned));
        let name = dotted
            .required()
            .iter()
            .find(|required| required.path == "service.name")
            .expect("service.name");
        assert_eq!(name.kind, Kind::String);
    }

    #[test]
    fn an_unknown_type_or_a_leaf_that_is_not_a_name_is_refused_at_bind_time() {
        let unknown = Layout::parse("port = \"number\"").expect_err("refused");
        assert!(unknown.message.contains("port asks for \"number\""));
        let not_a_name = Layout::parse("port = 80").expect_err("refused");
        assert!(not_a_name.message.contains("port is integer"));
        assert!(Layout::parse("= broken").is_err());
    }

    #[test]
    fn a_missing_key_and_a_key_of_the_wrong_type_are_named_by_their_path() {
        let layout = Layout::parse(
            "service.name = \"string\"\nservice.port = \"integer\"\nstarted = \"datetime\"",
        )
        .expect("a layout");
        let document: Table = "[service]\nname = 1\nport = 80".parse().expect("toml");
        let issues = layout.check(&document);
        assert_eq!(issues.len(), 2, "{issues:?}");
        assert_eq!(issues[0].code, "type");
        assert_eq!(issues[0].path.as_deref(), Some("service.name"));
        assert!(issues[0].message.contains("is integer"));
        assert_eq!(issues[1].code, "required");
        assert_eq!(issues[1].path.as_deref(), Some("started"));
        let held: Table = "started = 2026-09-10T08:00:00Z\n[service]\nname = \"a\"\nport = 1"
            .parse()
            .expect("toml");
        assert!(layout.check(&held).is_empty());
    }
}
