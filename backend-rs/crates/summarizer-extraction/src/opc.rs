use base64::{engine::general_purpose, Engine as _};
use image::{ImageFormat, ImageReader};
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

/// Maximum decoded pixel count for an embedded image before it is rejected as a
/// decompression bomb. Mirrors `summarizer-vision`'s `MAX_IMAGE_PIXELS` so the
/// extraction and vision paths enforce the same bound. 16M px ≈ 64 MB decoded (RGBA).
const MAX_IMAGE_PIXELS: u64 = 16_000_000;

/// Reject images whose header-declared dimensions would decode to more than
/// [`MAX_IMAGE_PIXELS`] pixels, before the unbounded `load_from_memory`
/// allocation. `into_dimensions` reads only the image header, so this is cheap
/// and runs ahead of any large allocation.
fn check_image_dimensions(bytes: &[u8], label: &str) -> Result<(), PipelineError> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|err| {
            PipelineError::Extraction(format!("Could not inspect {label} dimensions: {err}"))
        })?;
    let (width, height) = reader.into_dimensions().map_err(|err| {
        PipelineError::Extraction(format!("Could not inspect {label} dimensions: {err}"))
    })?;
    if u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS {
        return Err(PipelineError::Extraction(format!(
            "{label} dimensions {width}x{height} exceed limit of {MAX_IMAGE_PIXELS} pixels"
        )));
    }
    Ok(())
}

pub(crate) fn image_bytes_to_png_base64(
    bytes: &[u8],
    label: &str,
) -> Result<String, PipelineError> {
    check_image_dimensions(bytes, label)?;
    let image = image::load_from_memory(bytes)
        .map_err(|err| PipelineError::Extraction(format!("Could not decode {label}: {err}")))?;
    let mut png = Cursor::new(Vec::new());
    image
        .write_to(&mut png, ImageFormat::Png)
        .map_err(|err| PipelineError::Extraction(format!("Could not encode {label}: {err}")))?;
    Ok(general_purpose::STANDARD.encode(png.into_inner()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, GrayImage, ImageBuffer, Luma, Rgb, RgbImage};

    fn encode_png(image: DynamicImage) -> Vec<u8> {
        let mut png = Cursor::new(Vec::new());
        image.write_to(&mut png, ImageFormat::Png).unwrap();
        png.into_inner()
    }

    #[test]
    fn rejects_oversized_image_before_decode() {
        // 4001 x 4001 = 16,008,001 px, just over MAX_IMAGE_PIXELS (16,000,000).
        // Uses a single-channel image so the fixture buffer stays ~16 MB.
        let buffer: GrayImage = ImageBuffer::from_pixel(4001, 4001, Luma([0u8]));
        let bytes = encode_png(DynamicImage::ImageLuma8(buffer));
        let err = image_bytes_to_png_base64(&bytes, "test image").unwrap_err();
        match err {
            PipelineError::Extraction(message) => {
                assert!(
                    message.contains("exceed limit"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected Extraction error, got {other:?}"),
        }
    }

    #[test]
    fn accepts_reasonable_image() {
        let buffer: RgbImage = ImageBuffer::from_pixel(640, 480, Rgb([10, 20, 30]));
        let bytes = encode_png(DynamicImage::ImageRgb8(buffer));
        let encoded = image_bytes_to_png_base64(&bytes, "test image").unwrap();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn rejects_malformed_bytes_without_panic() {
        let err = image_bytes_to_png_base64(b"not an image", "test image").unwrap_err();
        assert!(matches!(err, PipelineError::Extraction(_)));
    }
}
