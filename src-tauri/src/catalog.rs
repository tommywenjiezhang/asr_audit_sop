//! Embedded SOP issue/template catalog (templates/catalog.json + HTML bodies + SOP images).

use include_dir::{include_dir, Dir};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;

pub static TEMPLATES_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/../templates");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Condition {
    #[serde(default)]
    pub field: String,
    pub op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDef {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_if: Option<Condition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueTypeRaw {
    pub id: String,
    pub name: String,
    pub sort: i64,
    pub sop_section: String,
    pub sop_page: String,
    pub description: String,
    pub fields: serde_json::Value,
    #[serde(default)]
    pub keywords: Vec<(String, f64)>,
    pub default_template: String,
    #[serde(default)]
    pub internal_steps: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueType {
    pub id: String,
    pub name: String,
    pub sort: i64,
    pub sop_section: String,
    pub sop_page: String,
    pub description: String,
    pub fields: Vec<FieldDef>,
    pub keywords: Vec<(String, f64)>,
    pub default_template: String,
    pub internal_steps: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateDef {
    pub id: String,
    pub issue_type: String,
    pub file: String,
    pub name: String,
    pub description: String,
    pub sop_section: String,
    pub sop_page: String,
    pub sop_status: String,
    pub version: i64,
    pub priority: i64,
    pub subject: String,
    #[serde(default)]
    pub required_fields: Vec<String>,
    #[serde(default)]
    pub optional_fields: Vec<String>,
    #[serde(default)]
    pub when: Vec<Condition>,
    #[serde(default)]
    pub asserts: Vec<Condition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaceholderDef {
    pub key: String,
    pub label: String,
    pub source: String,
    #[serde(default)]
    pub sample: String,
    #[serde(default)]
    pub verified_only: bool,
    #[serde(default)]
    pub exclude_values: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CatalogRaw {
    catalog_version: i64,
    sop_document: String,
    yes_no_unknown: Vec<String>,
    grade_options: Vec<String>,
    common_fields: Vec<FieldDef>,
    placeholders: Vec<PlaceholderDef>,
    issue_types: Vec<IssueTypeRaw>,
    field_sets: HashMap<String, Vec<FieldDef>>,
    templates: Vec<TemplateDef>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Catalog {
    pub catalog_version: i64,
    pub sop_document: String,
    pub yes_no_unknown: Vec<String>,
    pub grade_options: Vec<String>,
    pub common_fields: Vec<FieldDef>,
    pub placeholders: Vec<PlaceholderDef>,
    pub issue_types: Vec<IssueType>,
    pub templates: Vec<TemplateDef>,
}

impl Catalog {
    pub fn parse(json: &str) -> Result<Catalog, String> {
        let raw: CatalogRaw = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let mut issue_types = Vec::new();
        for it in raw.issue_types {
            let fields: Vec<FieldDef> = match &it.fields {
                serde_json::Value::String(s) if s.starts_with('@') => raw
                    .field_sets
                    .get(&s[1..])
                    .cloned()
                    .ok_or_else(|| format!("unknown field set {s} in {}", it.id))?,
                v => serde_json::from_value(v.clone()).map_err(|e| format!("{}: {e}", it.id))?,
            };
            issue_types.push(IssueType {
                id: it.id,
                name: it.name,
                sort: it.sort,
                sop_section: it.sop_section,
                sop_page: it.sop_page,
                description: it.description,
                fields,
                keywords: it.keywords,
                default_template: it.default_template,
                internal_steps: it.internal_steps,
            });
        }
        issue_types.sort_by_key(|i| i.sort);
        Ok(Catalog {
            catalog_version: raw.catalog_version,
            sop_document: raw.sop_document,
            yes_no_unknown: raw.yes_no_unknown,
            grade_options: raw.grade_options,
            common_fields: raw.common_fields,
            placeholders: raw.placeholders,
            issue_types,
            templates: raw.templates,
        })
    }

    pub fn issue(&self, id: &str) -> Option<&IssueType> {
        self.issue_types.iter().find(|i| i.id == id)
    }

    pub fn placeholder(&self, key: &str) -> Option<&PlaceholderDef> {
        self.placeholders.iter().find(|p| p.key == key)
    }

    /// All fields (common + issue-specific) for an issue type.
    pub fn fields_for(&self, issue_id: &str) -> Vec<FieldDef> {
        let mut v = self.common_fields.clone();
        if let Some(it) = self.issue(issue_id) {
            for f in &it.fields {
                if !v.iter().any(|x| x.key == f.key) {
                    v.push(f.clone());
                }
            }
        }
        v
    }

    pub fn field_def(&self, key: &str) -> Option<FieldDef> {
        self.common_fields
            .iter()
            .chain(self.issue_types.iter().flat_map(|i| i.fields.iter()))
            .find(|f| f.key == key)
            .cloned()
    }
}

pub static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    let json = TEMPLATES_DIR
        .get_file("catalog.json")
        .and_then(|f| f.contents_utf8())
        .expect("embedded catalog.json missing");
    Catalog::parse(json).expect("embedded catalog.json invalid")
});

pub fn catalog() -> &'static Catalog {
    &CATALOG
}

/// HTML body for a template file path (relative to templates/).
pub fn template_body(file: &str) -> Option<&'static str> {
    TEMPLATES_DIR.get_file(file).and_then(|f| f.contents_utf8())
}

/// Names of the embedded SOP images.
pub fn sop_images() -> Vec<(&'static str, &'static [u8])> {
    TEMPLATES_DIR
        .get_dir("sop-images")
        .map(|d| {
            d.files()
                .filter_map(|f| f.path().file_name().and_then(|n| n.to_str()).map(|n| (n, f.contents())))
                .collect()
        })
        .unwrap_or_default()
}
