use base64::{engine::general_purpose, Engine as _};
use std::io::{Cursor, Read, Write};
use summarizer_extraction::Extractor;
use summarizer_types::{PipelineConfig, VisionMode};

#[tokio::test]
async fn pptx_extraction_reads_slide_text_notes_and_tables() {
    let pptx = fixture_pptx();

    let output = Extractor::new()
        .extract_path(
            "doc_pptx".to_string(),
            &pptx,
            &PipelineConfig {
                skip_images: true,
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(output.document.total_pages, 6);
    assert_eq!(output.pages.len(), 6);
    assert!(output.pages[0].text.contains("Test Presentation"));
    assert!(output.pages[1].text.contains("[Speaker Notes]"));
    assert!(output.pages[1].text.contains("Remember to explain this"));

    let table = &output.pages[2].tables[0];
    assert_eq!(table[0], ["Header1", "Header2"]);
    assert_eq!(table[1], ["A", "B"]);
    assert_eq!(table[2], ["C", "D"]);
    assert!(!output.pages[2].text.contains("Header1"));

    assert_eq!(output.pages[5].tables.len(), 2);
    assert_eq!(output.pages[5].tables[0][0], ["T1-R1C1", "T1-R1C2"]);
    assert_eq!(output.pages[5].tables[1][0], ["T2-R1C1", "T2-R1C2"]);
}

#[tokio::test]
async fn pptx_extraction_respects_notes_and_table_skip_flags() {
    let pptx = fixture_pptx();

    let skip_notes = Extractor::new()
        .extract_path(
            "doc_pptx".to_string(),
            &pptx,
            &PipelineConfig {
                skip_tables: true,
                skip_images: true,
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();
    assert!(!skip_notes.pages[1].text.contains("[Speaker Notes]"));
    assert!(!skip_notes.pages[1]
        .text
        .contains("Remember to explain this"));
    assert!(!skip_notes.pages[2].tables.is_empty());

    let skip_slide_tables = Extractor::new()
        .extract_path(
            "doc_pptx".to_string(),
            &pptx,
            &PipelineConfig {
                skip_pptx_tables: true,
                skip_images: true,
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();
    assert!(skip_slide_tables.pages[1].text.contains("[Speaker Notes]"));
    assert!(skip_slide_tables.pages[2].tables.is_empty());
}

#[tokio::test]
async fn pptx_extraction_attaches_embedded_slide_images_for_vision() {
    let dir = tempfile::tempdir().unwrap();
    let pptx = dir.path().join("image_only.pptx");
    write_image_only_pptx(&pptx);

    let output = Extractor::new()
        .extract_path(
            "doc_image_pptx".to_string(),
            &pptx,
            &PipelineConfig {
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(output.pages.len(), 1);
    assert_eq!(output.pages[0].text, "");
    let image_base64 = output.pages[0].image_base64.as_deref().unwrap();
    let image_bytes = general_purpose::STANDARD.decode(image_base64).unwrap();
    image::load_from_memory(&image_bytes).unwrap();
}

#[tokio::test]
async fn pptx_extraction_resolves_absolute_relationship_targets_and_joins_text_runs() {
    let dir = tempfile::tempdir().unwrap();
    let pptx = dir.path().join("absolute_targets.pptx");
    write_absolute_target_pptx(&pptx);

    let output = Extractor::new()
        .extract_path(
            "doc_absolute_pptx".to_string(),
            &pptx,
            &PipelineConfig {
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(output.pages.len(), 1);
    assert!(output.pages[0].text.contains("Split run sentence"));
    assert!(!output.pages[0].text.contains("Split\nrun\nsentence"));
    let image_base64 = output.pages[0].image_base64.as_deref().unwrap();
    let image_bytes = general_purpose::STANDARD.decode(image_base64).unwrap();
    image::load_from_memory(&image_bytes).unwrap();
}

#[tokio::test]
async fn pptx_extraction_respects_skip_images_for_embedded_slide_images() {
    let dir = tempfile::tempdir().unwrap();
    let pptx = dir.path().join("image_only.pptx");
    write_image_only_pptx(&pptx);

    let output = Extractor::new()
        .extract_path(
            "doc_image_pptx".to_string(),
            &pptx,
            &PipelineConfig {
                skip_images: true,
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(output.pages[0].image_base64, None);
}

#[tokio::test]
async fn pptx_extraction_renders_every_slide_for_vision() {
    if !soffice_available() {
        return;
    }

    let output = Extractor::new()
        .extract_path(
            "doc_pptx".to_string(),
            &fixture_pptx(),
            &PipelineConfig {
                vision_mode: VisionMode::Openai,
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(output.pages.len(), 6);
    for page in &output.pages {
        let image_base64 = page.image_base64.as_deref().unwrap();
        let image_bytes = general_purpose::STANDARD.decode(image_base64).unwrap();
        image::load_from_memory(&image_bytes).unwrap();
    }
}

#[tokio::test]
async fn pptx_extraction_renders_hidden_slides_for_vision() {
    if !soffice_available() {
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let pptx = dir.path().join("hidden_slide.pptx");
    write_hidden_slide_copy(&fixture_pptx(), &pptx, "ppt/slides/slide2.xml");

    let output = Extractor::new()
        .extract_path(
            "doc_hidden_pptx".to_string(),
            &pptx,
            &PipelineConfig {
                vision_mode: VisionMode::Openai,
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(output.document.total_pages, 6);
    assert_eq!(output.pages.len(), 6);
    assert!(output.pages[1].text.contains("Remember to explain this"));
    for page in &output.pages {
        let image_base64 = page.image_base64.as_deref().unwrap();
        let image_bytes = general_purpose::STANDARD.decode(image_base64).unwrap();
        image::load_from_memory(&image_bytes).unwrap();
    }
}

#[tokio::test]
async fn pptx_extraction_rejects_inflated_entry_beyond_declared_size() {
    let dir = tempfile::tempdir().unwrap();
    let pptx = dir.path().join("inflated_entry.pptx");
    write_pptx_with_declared_uncompressed_size(&pptx, "ppt/presentation.xml", 1, 4096);

    let error = Extractor::new()
        .extract_path(
            "doc_inflated_pptx".to_string(),
            &pptx,
            &PipelineConfig {
                skip_images: true,
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("inflated beyond declared size"));
}

#[tokio::test]
async fn pptx_extraction_errors_on_missing_required_part() {
    let dir = tempfile::tempdir().unwrap();
    let pptx = dir.path().join("missing_presentation.pptx");
    let file = std::fs::File::create(&pptx).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    write_zip_entry(
        &mut archive,
        options,
        "ppt/_rels/presentation.xml.rels",
        br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>"#,
    );
    archive.finish().unwrap();

    let error = Extractor::new()
        .extract_path(
            "doc_missing_pptx".to_string(),
            &pptx,
            &PipelineConfig {
                skip_images: true,
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("Missing PPTX entry ppt/presentation.xml"));
}

#[tokio::test]
async fn pptx_extraction_errors_on_truncated_archive() {
    let dir = tempfile::tempdir().unwrap();
    let pptx = dir.path().join("truncated.pptx");
    std::fs::write(&pptx, b"PK\x03\x04truncated").unwrap();

    let error = Extractor::new()
        .extract_path(
            "doc_truncated_pptx".to_string(),
            &pptx,
            &PipelineConfig {
                skip_images: true,
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("Invalid PPTX archive"));
}

fn fixture_pptx() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap()
        .join("backend-rs")
        .join("tests")
        .join("fixtures")
        .join("golden")
        .join("test_presentation.pptx")
}

fn soffice_available() -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg("command -v soffice >/dev/null 2>&1 || command -v libreoffice >/dev/null 2>&1 || test -x /Applications/LibreOffice.app/Contents/MacOS/soffice || test -x /opt/homebrew/bin/soffice || test -x /usr/local/bin/soffice")
        .status()
        .is_ok_and(|status| status.success())
}

fn write_image_only_pptx(path: &std::path::Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    write_zip_entry(
        &mut archive,
        options,
        "ppt/presentation.xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst></p:presentation>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "ppt/_rels/presentation.xml.rels",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/></Relationships>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "ppt/slides/slide1.xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:pic><p:blipFill><a:blip r:embed="rId2"/></p:blipFill></p:pic></p:spTree></p:cSld></p:sld>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "ppt/slides/_rels/slide1.xml.rels",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/></Relationships>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "ppt/media/image1.png",
        &sample_png_bytes(),
    );
    archive.finish().unwrap();
}

fn write_absolute_target_pptx(path: &std::path::Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    write_zip_entry(
        &mut archive,
        options,
        "ppt/presentation.xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst></p:presentation>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "ppt/_rels/presentation.xml.rels",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="/ppt/slides/slide1.xml"/></Relationships>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "ppt/slides/slide1.xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:bodyPr/><a:p><a:r><a:t>Split </a:t></a:r><a:r><a:t>run </a:t></a:r><a:r><a:t>sentence</a:t></a:r></a:p></p:txBody></p:sp><p:pic><p:blipFill><a:blip r:embed="rIdImage"/></p:blipFill></p:pic></p:spTree></p:cSld></p:sld>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "ppt/slides/_rels/slide1.xml.rels",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdImage" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="/ppt/media/image1.png"/></Relationships>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "ppt/media/image1.png",
        &sample_png_bytes(),
    );
    archive.finish().unwrap();
}

fn write_hidden_slide_copy(input: &std::path::Path, output: &std::path::Path, hidden_slide: &str) {
    let input_file = std::fs::File::open(input).unwrap();
    let mut input_archive = zip::ZipArchive::new(input_file).unwrap();
    let output_file = std::fs::File::create(output).unwrap();
    let mut output_archive = zip::ZipWriter::new(output_file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    for index in 0..input_archive.len() {
        let mut entry = input_archive.by_index(index).unwrap();
        let name = entry.name().to_string();
        if entry.is_dir() {
            output_archive.add_directory(name, options).unwrap();
            continue;
        }

        let mut contents = Vec::new();
        entry.read_to_end(&mut contents).unwrap();
        if name == hidden_slide {
            let xml = String::from_utf8(contents).unwrap();
            contents = xml
                .replacen("<p:sld ", r#"<p:sld show="0" "#, 1)
                .into_bytes();
        }

        output_archive.start_file(name, options).unwrap();
        output_archive.write_all(&contents).unwrap();
    }

    output_archive.finish().unwrap();
}

fn write_pptx_with_declared_uncompressed_size(
    path: &std::path::Path,
    entry_name: &str,
    declared_size: u32,
    actual_size: usize,
) {
    let mut buffer = Cursor::new(Vec::new());
    {
        let mut archive = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        archive.start_file(entry_name, options).unwrap();
        archive.write_all(&vec![b'a'; actual_size]).unwrap();
        archive.finish().unwrap();
    }
    let mut bytes = buffer.into_inner();
    patch_zip_uncompressed_sizes(&mut bytes, declared_size);
    std::fs::write(path, bytes).unwrap();
}

fn patch_zip_uncompressed_sizes(bytes: &mut [u8], declared_size: u32) {
    let mut index = 0;
    while index + 30 <= bytes.len() {
        let signature = u32::from_le_bytes(bytes[index..index + 4].try_into().unwrap());
        match signature {
            0x0403_4b50 => {
                bytes[index + 22..index + 26].copy_from_slice(&declared_size.to_le_bytes());
                index += 30;
            }
            0x0201_4b50 => {
                bytes[index + 24..index + 28].copy_from_slice(&declared_size.to_le_bytes());
                index += 46;
            }
            _ => index += 1,
        }
    }
}

fn sample_png_bytes() -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn write_zip_entry(
    archive: &mut zip::ZipWriter<std::fs::File>,
    options: zip::write::SimpleFileOptions,
    path: &str,
    contents: &[u8],
) {
    archive.start_file(path, options).unwrap();
    archive.write_all(contents).unwrap();
}
