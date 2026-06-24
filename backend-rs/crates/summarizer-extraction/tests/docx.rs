use base64::{engine::general_purpose, Engine as _};
use std::io::{Cursor, Write};
use summarizer_extraction::{DocumentKind, Extractor};
use summarizer_types::PipelineConfig;

#[tokio::test]
async fn docx_extraction_reads_rich_word_content() {
    let dir = tempfile::tempdir().unwrap();
    let docx = dir.path().join("rich.docx");
    write_rich_docx(&docx);

    let output = Extractor::new()
        .extract_path(
            "doc_docx".to_string(),
            &docx,
            &PipelineConfig {
                run_summarization: false,
                keep_base64_images: true,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(output.document.filename, "rich.docx");
    assert_eq!(
        output.document.metadata["source_type"],
        serde_json::json!("docx")
    );
    assert_eq!(
        output.document.metadata["file_type"],
        serde_json::json!(".docx")
    );
    assert_eq!(output.pages.len(), 1);

    let page = &output.pages[0];
    assert!(page.text.contains("# Executive Summary"));
    assert!(page
        .text
        .contains("This is important and includes Example."));
    assert!(page.text.contains("1. First item"));
    assert!(page.text.contains("[Footnote] Footnote detail"));
    assert!(page.text.contains("[Comment] Review comment"));
    assert!(page.text.contains("[Header] Confidential header"));
    assert!(page.text.contains("[Footer] Page footer"));

    assert_eq!(page.tables.len(), 1);
    assert_eq!(page.tables[0][0], ["Metric", "Value"]);
    assert_eq!(page.tables[0][1], ["Accuracy", "98%"]);

    assert_eq!(page.embedded_images.len(), 1);
    assert_eq!(
        page.embedded_images[0].alt_text.as_deref(),
        Some("Architecture diagram")
    );
    assert!(page.embedded_images[0].base64.is_some());
    let png = page.image_base64.as_deref().unwrap();
    image::load_from_memory(&general_purpose::STANDARD.decode(png).unwrap()).unwrap();
    assert!(page.html.is_none());
}

#[tokio::test]
async fn docx_extraction_respects_table_and_image_skip_flags() {
    let dir = tempfile::tempdir().unwrap();
    let docx = dir.path().join("rich.docx");
    write_rich_docx(&docx);

    let output = Extractor::new()
        .extract_path(
            "doc_docx_skip".to_string(),
            &docx,
            &PipelineConfig {
                skip_tables: true,
                skip_images: true,
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();

    let page = &output.pages[0];
    assert!(page.tables.is_empty());
    assert!(page.image_base64.is_none());
    assert!(page.embedded_images.is_empty());
    assert!(!page.text.contains("Accuracy | 98%"));
}

#[tokio::test]
async fn docx_extraction_splits_on_word_page_breaks_and_strips_nested_base64() {
    let dir = tempfile::tempdir().unwrap();
    let docx = dir.path().join("paged.docx");
    write_paged_docx(&docx);

    let output = Extractor::new()
        .extract_path(
            "doc_docx_pages".to_string(),
            &docx,
            &PipelineConfig {
                run_summarization: false,
                keep_base64_images: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(
        output.document.metadata["chunking_strategy"],
        serde_json::json!("docx-page-breaks")
    );
    assert_eq!(output.pages.len(), 3);
    assert!(output.pages[0].text.contains("# Page One"));
    assert!(!output.pages[0].text.contains("# Page Two"));
    assert!(output.pages[1].text.contains("# Page Two"));
    assert!(!output.pages[1].text.contains("# Page Three"));
    assert!(output.pages[2].text.contains("# Page Three"));
    assert!(!output
        .pages
        .iter()
        .any(|page| page.text.contains("[Page break]")));

    let first_page = &output.pages[0];
    assert_eq!(first_page.embedded_images.len(), 1);
    assert!(first_page.embedded_images[0].base64.is_none());
}

#[test]
fn document_kind_accepts_docx_but_not_legacy_doc() {
    assert_eq!(
        DocumentKind::from_path(std::path::Path::new("proposal.docx")).unwrap(),
        DocumentKind::Docx
    );
    assert!(DocumentKind::from_path(std::path::Path::new("legacy.doc")).is_err());
}

#[tokio::test]
async fn docx_extraction_preserves_multi_paragraph_table_cells_and_notes() {
    let dir = tempfile::tempdir().unwrap();
    let docx = dir.path().join("multi_paragraph.docx");
    write_multi_paragraph_docx(&docx);

    let output = Extractor::new()
        .extract_path(
            "doc_docx_multi".to_string(),
            &docx,
            &PipelineConfig {
                run_summarization: false,
                ..PipelineConfig::default()
            },
        )
        .await
        .unwrap();

    let page = &output.pages[0];
    assert_eq!(page.tables[0][0][0], "Revenue\n2024");
    assert!(page
        .text
        .contains("[Footnote] First note paragraph\nSecond note paragraph"));
    assert!(!page.text.contains("Revenue2024"));
    assert!(!page.text.contains("paragraphSecond"));
}

fn write_paged_docx(path: &std::path::Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    write_zip_entry(
        &mut archive,
        options,
        "[Content_Types].xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Default Extension="png" ContentType="image/png"/>
</Types>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "word/_rels/document.xml.rels",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdImage" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="/word/media/image1.png"/>
</Relationships>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "word/styles.xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/></w:style>
</w:styles>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "word/document.xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
  xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
  xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
  xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing">
  <w:body>
    <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Page One</w:t></w:r></w:p>
    <w:p><w:r><w:t>First page body.</w:t></w:r></w:p>
    <w:p><w:r><w:drawing><wp:inline><wp:docPr id="1" name="Picture 1" descr="Page image"/><a:graphic><a:graphicData><a:blip r:embed="rIdImage"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>
    <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:lastRenderedPageBreak/><w:t>Page Two</w:t></w:r></w:p>
    <w:p><w:r><w:t>Second page body.</w:t><w:br w:type="page"/></w:r></w:p>
    <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Page Three</w:t></w:r></w:p>
    <w:p><w:r><w:t>Third page body.</w:t></w:r></w:p>
    <w:sectPr/>
  </w:body>
</w:document>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "word/media/image1.png",
        &sample_png_bytes(),
    );

    archive.finish().unwrap();
}

fn write_multi_paragraph_docx(path: &std::path::Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    write_zip_entry(
        &mut archive,
        options,
        "[Content_Types].xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
</Types>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "word/footnotes.xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:footnote w:id="2"><w:p><w:r><w:t>First note paragraph</w:t></w:r></w:p><w:p><w:r><w:t>Second note paragraph</w:t></w:r></w:p></w:footnote>
</w:footnotes>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "word/document.xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>Document body</w:t><w:footnoteReference w:id="2"/></w:r></w:p>
    <w:tbl><w:tr><w:tc><w:p><w:r><w:t>Revenue</w:t></w:r></w:p><w:p><w:r><w:t>2024</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
  </w:body>
</w:document>"#,
    );

    archive.finish().unwrap();
}

fn write_rich_docx(path: &std::path::Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    write_zip_entry(
        &mut archive,
        options,
        "[Content_Types].xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Default Extension="png" ContentType="image/png"/>
</Types>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "word/_rels/document.xml.rels",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdHyperlink" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com" TargetMode="External"/>
  <Relationship Id="rIdImage" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="/word/media/image1.png"/>
</Relationships>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "word/styles.xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/></w:style>
</w:styles>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "word/numbering.xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:abstractNum w:abstractNumId="7">
    <w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl>
  </w:abstractNum>
  <w:num w:numId="1"><w:abstractNumId w:val="7"/></w:num>
</w:numbering>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "word/header1.xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:p><w:r><w:t>Confidential header</w:t></w:r></w:p>
</w:hdr>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "word/footer1.xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:ftr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:p><w:r><w:t>Page footer</w:t></w:r></w:p>
</w:ftr>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "word/footnotes.xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:footnote w:id="2"><w:p><w:r><w:t>Footnote detail</w:t></w:r></w:p></w:footnote>
</w:footnotes>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "word/comments.xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:comments xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:comment w:id="3"><w:p><w:r><w:t>Review comment</w:t></w:r></w:p></w:comment>
</w:comments>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "word/document.xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
  xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
  xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
  xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing">
  <w:body>
    <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Executive Summary</w:t></w:r></w:p>
    <w:p>
      <w:r><w:t xml:space="preserve">This is </w:t></w:r>
      <w:r><w:rPr><w:b/><w:i/></w:rPr><w:t>important</w:t></w:r>
      <w:r><w:t xml:space="preserve"> and includes </w:t></w:r>
      <w:hyperlink r:id="rIdHyperlink"><w:r><w:t>Example</w:t></w:r></w:hyperlink>
      <w:r><w:t>.</w:t></w:r>
    </w:p>
    <w:p>
      <w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr>
      <w:r><w:t>First item</w:t></w:r>
    </w:p>
    <w:tbl>
      <w:tr><w:tc><w:p><w:r><w:t>Metric</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Value</w:t></w:r></w:p></w:tc></w:tr>
      <w:tr><w:tc><w:p><w:r><w:t>Accuracy</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>98%</w:t></w:r></w:p></w:tc></w:tr>
    </w:tbl>
    <w:p><w:r><w:t>Footnote marker</w:t><w:footnoteReference w:id="2"/></w:r></w:p>
    <w:p><w:r><w:t>Comment marker</w:t><w:commentReference w:id="3"/></w:r></w:p>
    <w:p><w:r><w:drawing><wp:inline><wp:docPr id="1" name="Picture 1" descr="Architecture diagram"/><a:graphic><a:graphicData><a:blip r:embed="rIdImage"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>
    <w:sectPr/>
  </w:body>
</w:document>"#,
    );
    write_zip_entry(
        &mut archive,
        options,
        "word/media/image1.png",
        &sample_png_bytes(),
    );

    archive.finish().unwrap();
}

fn sample_png_bytes() -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 128, 255, 255]));
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
