use base64::{engine::general_purpose, Engine as _};
use image::ImageFormat;
use std::{collections::HashMap, io::Cursor};
use summarizer_types::PipelineError;

#[derive(Debug, Clone)]
pub(crate) struct Relationship {
    pub(crate) target: String,
    pub(crate) relationship_type: Option<String>,
    pub(crate) target_mode: Option<String>,
}

impl Relationship {
    pub(crate) fn is_external(&self) -> bool {
        self.target_mode.as_deref() == Some("External")
    }

    pub(crate) fn type_ends_with(&self, suffix: &str) -> bool {
        self.relationship_type
            .as_deref()
            .is_some_and(|relationship_type| relationship_type.ends_with(suffix))
    }
}

pub(crate) fn parse_relationships(
    xml: &str,
    label: &str,
) -> Result<HashMap<String, Relationship>, PipelineError> {
    let document = roxmltree::Document::parse(xml).map_err(|err| {
        PipelineError::Extraction(format!("Invalid {label} relationship XML: {err}"))
    })?;
    let mut relationships = HashMap::new();
    for relationship in document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "Relationship")
    {
        let Some(id) = relationship.attribute("Id") else {
            continue;
        };
        let Some(target) = relationship.attribute("Target") else {
            continue;
        };
        relationships.insert(
            id.to_string(),
            Relationship {
                target: target.to_string(),
                relationship_type: relationship.attribute("Type").map(ToString::to_string),
                target_mode: relationship
                    .attribute("TargetMode")
                    .map(ToString::to_string),
            },
        );
    }
    Ok(relationships)
}

pub(crate) fn normalize_package_path(base_dir: &str, target: &str) -> String {
    if let Some(target) = target.strip_prefix('/') {
        return target.to_string();
    }

    let mut parts = Vec::new();
    for part in base_dir.split('/').chain(target.split('/')) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

pub(crate) fn image_bytes_to_png_base64(
    bytes: &[u8],
    label: &str,
) -> Result<String, PipelineError> {
    let image = image::load_from_memory(bytes)
        .map_err(|err| PipelineError::Extraction(format!("Could not decode {label}: {err}")))?;
    let mut png = Cursor::new(Vec::new());
    image
        .write_to(&mut png, ImageFormat::Png)
        .map_err(|err| PipelineError::Extraction(format!("Could not encode {label}: {err}")))?;
    Ok(general_purpose::STANDARD.encode(png.into_inner()))
}
