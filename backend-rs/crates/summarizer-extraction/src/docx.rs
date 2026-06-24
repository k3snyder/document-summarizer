use base64::{engine::general_purpose, Engine as _};
use roxmltree::Node;
use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    path::Path,
};
use summarizer_types::{
    DocumentMetadata, DocumentOutput, ExtractedImage, PageOutput, PipelineConfig, PipelineError,
    TableCell,
};

use super::{
    emit_extraction_progress, opc, split_recursive, ExtractionProgress, SizeLimitedZipArchive,
};

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

#[derive(Debug, Default)]
struct DocxContext {
    relationships: HashMap<String, opc::Relationship>,
    styles: HashMap<String, StyleInfo>,
    numbering: HashMap<(String, String), String>,
    footnotes: HashMap<String, String>,
    endnotes: HashMap<String, String>,
    comments: HashMap<String, String>,
    media: HashMap<String, Vec<u8>>,
    content_types: HashMap<String, String>,
    include_tables: bool,
    include_images: bool,
}

#[derive(Debug, Clone, Default)]
struct StyleInfo {
    heading_level: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocxBlockKind {
    Paragraph,
    Heading,
    ListItem,
    Table,
    Image,
    Header,
    Footer,
    Footnote,
    Endnote,
    Comment,
    PageBreak,
    SectionBreak,
}

#[derive(Debug, Clone)]
struct DocxBlock {
    kind: DocxBlockKind,
    text: String,
    level: Option<u8>,
    list_format: Option<String>,
    table: Option<Vec<Vec<TableCell>>>,
    image: Option<ExtractedImage>,
}

#[derive(Debug, Default)]
struct ParseState {
    image_index: usize,
}

impl ParseState {
    fn next_image_id(&mut self) -> String {
        self.image_index += 1;
        format!("image_{}", self.image_index)
    }
}

#[derive(Debug, Clone)]
struct ParsedRun {
    text: String,
    images: Vec<ExtractedImage>,
    footnote_refs: Vec<String>,
    endnote_refs: Vec<String>,
    comment_refs: Vec<String>,
    page_breaks: usize,
}

impl ParsedRun {
    fn empty() -> Self {
        Self {
            text: String::new(),
            images: Vec::new(),
            footnote_refs: Vec::new(),
            endnote_refs: Vec::new(),
            comment_refs: Vec::new(),
            page_breaks: 0,
        }
    }

    fn extend(&mut self, other: Self) {
        self.text.push_str(&other.text);
        self.images.extend(other.images);
        self.footnote_refs.extend(other.footnote_refs);
        self.endnote_refs.extend(other.endnote_refs);
        self.comment_refs.extend(other.comment_refs);
        self.page_breaks += other.page_breaks;
    }
}

pub(super) fn extract_docx_document_with_progress(
    document_id: String,
    filename: String,
    path: &Path,
    config: &PipelineConfig,
    progress: &(dyn Fn(ExtractionProgress) + Send + Sync),
) -> Result<DocumentOutput, PipelineError> {
    let file = File::open(path)
        .map_err(|err| PipelineError::Extraction(format!("Could not open DOCX: {err}")))?;
    let mut archive = SizeLimitedZipArchive::new(file, "DOCX")?;
    let entry_names = zip_entry_names(&mut archive);
    let document_xml = read_zip_string(&mut archive, "word/document.xml")?;
    let context = build_context(&mut archive, &entry_names, config)?;
    let mut state = ParseState::default();

    emit_extraction_progress(progress, 1, 1, 0, "Extracting Word document content.");

    let mut blocks = Vec::new();
    blocks.extend(parse_header_footer_blocks(
        &mut archive,
        &entry_names,
        &context,
        &mut state,
        DocxBlockKind::Header,
        "word/header",
    )?);
    blocks.extend(parse_document_blocks(&document_xml, &context, &mut state)?);
    blocks.extend(parse_header_footer_blocks(
        &mut archive,
        &entry_names,
        &context,
        &mut state,
        DocxBlockKind::Footer,
        "word/footer",
    )?);
    let blocks = split_oversized_blocks(blocks, config.chunk_size);
    let (chunks, chunking_strategy) =
        chunks_from_page_breaks(blocks.clone()).unwrap_or_else(|| {
            (
                chunk_blocks(blocks, config.chunk_size),
                "docx-structural".to_string(),
            )
        });
    let total_pages = chunks.len().max(1);
    let mut pages = Vec::with_capacity(total_pages);

    for (index, chunk) in chunks.into_iter().enumerate() {
        let page_number = index + 1;
        emit_extraction_progress(
            progress,
            page_number,
            total_pages,
            index,
            format!("Preparing DOCX chunk {page_number} of {total_pages}."),
        );
        pages.push(chunk_to_page(page_number, filename.clone(), chunk, config));
        emit_extraction_progress(
            progress,
            page_number,
            total_pages,
            index + 1,
            format!("Prepared DOCX chunk {page_number} of {total_pages}."),
        );
    }

    if pages.is_empty() {
        pages.push(chunk_to_page(1, filename.clone(), Vec::new(), config));
    }

    Ok(DocumentOutput {
        document: DocumentMetadata {
            document_id,
            filename,
            total_pages: pages.len(),
            metadata: [
                ("source_type".to_string(), serde_json::json!("docx")),
                ("file_type".to_string(), serde_json::json!(".docx")),
                (
                    "chunking_strategy".to_string(),
                    serde_json::json!(chunking_strategy),
                ),
            ]
            .into_iter()
            .collect(),
        },
        pages,
        metrics: None,
    })
}

fn build_context(
    archive: &mut SizeLimitedZipArchive,
    entry_names: &[String],
    config: &PipelineConfig,
) -> Result<DocxContext, PipelineError> {
    let relationships = read_optional_zip_string(archive, "word/_rels/document.xml.rels")?
        .map(|xml| opc::parse_relationships(&xml, "DOCX"))
        .transpose()?
        .unwrap_or_default();
    let styles = read_optional_zip_string(archive, "word/styles.xml")?
        .map(|xml| parse_styles(&xml))
        .transpose()?
        .unwrap_or_default();
    let numbering = read_optional_zip_string(archive, "word/numbering.xml")?
        .map(|xml| parse_numbering(&xml))
        .transpose()?
        .unwrap_or_default();
    let footnotes = read_optional_zip_string(archive, "word/footnotes.xml")?
        .map(|xml| parse_notes(&xml, "footnote"))
        .transpose()?
        .unwrap_or_default();
    let endnotes = read_optional_zip_string(archive, "word/endnotes.xml")?
        .map(|xml| parse_notes(&xml, "endnote"))
        .transpose()?
        .unwrap_or_default();
    let comments = read_optional_zip_string(archive, "word/comments.xml")?
        .map(|xml| parse_notes(&xml, "comment"))
        .transpose()?
        .unwrap_or_default();
    let content_types = read_optional_zip_string(archive, "[Content_Types].xml")?
        .map(|xml| parse_content_types(&xml))
        .transpose()?
        .unwrap_or_default();
    let media = read_media_entries(archive, entry_names)?;

    Ok(DocxContext {
        relationships,
        styles,
        numbering,
        footnotes,
        endnotes,
        comments,
        media,
        content_types,
        include_tables: !config.skip_tables && !config.text_only,
        include_images: !config.skip_images && !config.text_only,
    })
}

fn parse_document_blocks(
    xml: &str,
    context: &DocxContext,
    state: &mut ParseState,
) -> Result<Vec<DocxBlock>, PipelineError> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|err| PipelineError::Extraction(format!("Invalid DOCX document XML: {err}")))?;
    let body = document
        .descendants()
        .find(|node| is_element_named(*node, "body"))
        .ok_or_else(|| {
            PipelineError::Extraction("DOCX document is missing word/body".to_string())
        })?;
    parse_block_children(body, context, state, None)
}

fn parse_header_footer_blocks(
    archive: &mut SizeLimitedZipArchive,
    entry_names: &[String],
    context: &DocxContext,
    state: &mut ParseState,
    kind: DocxBlockKind,
    prefix: &str,
) -> Result<Vec<DocxBlock>, PipelineError> {
    let mut paths = entry_names
        .iter()
        .filter(|name| name.starts_with(prefix) && name.ends_with(".xml"))
        .cloned()
        .collect::<Vec<_>>();
    paths.sort();

    let mut blocks = Vec::new();
    for path in paths {
        let xml = read_zip_string(archive, &path)?;
        let document = roxmltree::Document::parse(&xml)
            .map_err(|err| PipelineError::Extraction(format!("Invalid DOCX part {path}: {err}")))?;
        let part_blocks =
            parse_block_children(document.root_element(), context, state, Some(kind))?;
        blocks.extend(part_blocks);
    }
    Ok(blocks)
}

fn parse_block_children(
    container: Node<'_, '_>,
    context: &DocxContext,
    state: &mut ParseState,
    forced_kind: Option<DocxBlockKind>,
) -> Result<Vec<DocxBlock>, PipelineError> {
    let mut blocks = Vec::new();
    for child in container.children().filter(|node| node.is_element()) {
        match child.tag_name().name() {
            "p" => blocks.extend(parse_paragraph(child, context, state, forced_kind)?),
            "tbl" if context.include_tables => {
                blocks.push(parse_table(child, context, forced_kind)?)
            }
            "sectPr" => blocks.push(DocxBlock {
                kind: DocxBlockKind::SectionBreak,
                text: String::new(),
                level: None,
                table: None,
                image: None,
                list_format: None,
            }),
            _ => {}
        }
    }
    Ok(blocks)
}

fn parse_paragraph(
    paragraph: Node<'_, '_>,
    context: &DocxContext,
    state: &mut ParseState,
    forced_kind: Option<DocxBlockKind>,
) -> Result<Vec<DocxBlock>, PipelineError> {
    let paragraph_props = paragraph
        .children()
        .find(|node| is_element_named(*node, "pPr"));
    let style_id = paragraph_props.and_then(paragraph_style_id);
    let style = style_id.and_then(|id| context.styles.get(id));
    let list_info = paragraph_props.and_then(|properties| paragraph_list_info(properties, context));

    let mut parsed = ParsedRun::empty();
    for child in paragraph.children().filter(|node| node.is_element()) {
        match child.tag_name().name() {
            "r" => parsed.extend(parse_run(child, context, state)?),
            "hyperlink" => parsed.extend(parse_hyperlink(child, context, state)?),
            _ => {}
        }
    }

    let paragraph_text = parsed.text.trim().to_string();
    let mut blocks = Vec::new();
    let kind = forced_kind.unwrap_or_else(|| {
        if list_info.is_some() {
            DocxBlockKind::ListItem
        } else if style.and_then(|style| style.heading_level).is_some() {
            DocxBlockKind::Heading
        } else {
            DocxBlockKind::Paragraph
        }
    });
    let level = match kind {
        DocxBlockKind::Heading => style.and_then(|style| style.heading_level),
        DocxBlockKind::ListItem => list_info.as_ref().map(|info| info.level.saturating_add(1)),
        _ => None,
    };
    let leading_page_breaks = leading_page_break_count(paragraph).min(parsed.page_breaks);

    for _ in 0..leading_page_breaks {
        blocks.push(page_break_block());
    }

    if !paragraph_text.is_empty() || !parsed.images.is_empty() {
        blocks.push(DocxBlock {
            kind,
            text: paragraph_text,
            level,
            table: None,
            image: None,
            list_format: list_info.as_ref().map(|info| info.format.clone()),
        });
    }

    for image in parsed.images {
        blocks.push(DocxBlock {
            kind: DocxBlockKind::Image,
            text: image
                .alt_text
                .clone()
                .or_else(|| image.filename.clone())
                .unwrap_or_else(|| image.id.clone()),
            level: None,
            table: None,
            image: Some(image),
            list_format: None,
        });
    }

    for _ in leading_page_breaks..parsed.page_breaks {
        blocks.push(page_break_block());
    }

    append_note_blocks(
        &mut blocks,
        DocxBlockKind::Footnote,
        &parsed.footnote_refs,
        &context.footnotes,
    );
    append_note_blocks(
        &mut blocks,
        DocxBlockKind::Endnote,
        &parsed.endnote_refs,
        &context.endnotes,
    );
    append_note_blocks(
        &mut blocks,
        DocxBlockKind::Comment,
        &parsed.comment_refs,
        &context.comments,
    );

    Ok(blocks)
}

fn parse_hyperlink(
    hyperlink: Node<'_, '_>,
    context: &DocxContext,
    state: &mut ParseState,
) -> Result<ParsedRun, PipelineError> {
    let mut parsed = ParsedRun::empty();
    for child in hyperlink.children().filter(|node| node.is_element()) {
        if is_element_named(child, "r") {
            parsed.extend(parse_run(child, context, state)?);
        }
    }
    Ok(parsed)
}

fn parse_run(
    run: Node<'_, '_>,
    context: &DocxContext,
    state: &mut ParseState,
) -> Result<ParsedRun, PipelineError> {
    let mut parsed = ParsedRun::empty();

    for child in run.children().filter(|node| node.is_element()) {
        match child.tag_name().name() {
            "lastRenderedPageBreak" => parsed.page_breaks += 1,
            "t" => {
                let Some(text) = child.text().filter(|text| !text.is_empty()) else {
                    continue;
                };
                parsed.text.push_str(text);
            }
            "tab" => parsed.text.push('\t'),
            "br" | "cr" => {
                if attr(child, W_NS, "type") == Some("page") {
                    parsed.page_breaks += 1;
                }
                parsed.text.push('\n');
            }
            "drawing" | "pict" => parsed.images.extend(extract_images(child, context, state)?),
            "footnoteReference" => {
                if let Some(id) = attr(child, W_NS, "id") {
                    parsed.footnote_refs.push(id.to_string());
                    parsed.text.push_str(&format!("[Footnote {id}]"));
                }
            }
            "endnoteReference" => {
                if let Some(id) = attr(child, W_NS, "id") {
                    parsed.endnote_refs.push(id.to_string());
                    parsed.text.push_str(&format!("[Endnote {id}]"));
                }
            }
            "commentReference" => {
                if let Some(id) = attr(child, W_NS, "id") {
                    parsed.comment_refs.push(id.to_string());
                    parsed.text.push_str(&format!("[Comment {id}]"));
                }
            }
            _ => {}
        }
    }

    Ok(parsed)
}

fn extract_images(
    container: Node<'_, '_>,
    context: &DocxContext,
    state: &mut ParseState,
) -> Result<Vec<ExtractedImage>, PipelineError> {
    if !context.include_images {
        return Ok(Vec::new());
    }

    let mut images = Vec::new();
    let alt_text = container
        .descendants()
        .find(|node| is_element_named(*node, "docPr"))
        .and_then(|node| node.attribute("descr").or_else(|| node.attribute("title")))
        .map(ToString::to_string);

    for blip in container
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "blip")
    {
        let Some(rel_id) = attr(blip, R_NS, "embed").or_else(|| attr(blip, R_NS, "link")) else {
            continue;
        };
        let Some(relationship) = context.relationships.get(rel_id) else {
            continue;
        };
        if !relationship.type_ends_with("/image") {
            continue;
        }
        if relationship.is_external() {
            continue;
        }

        let path = opc::normalize_package_path("word", &relationship.target);
        let bytes = context.media.get(&path);
        let content_type = content_type_for_path(&path, &context.content_types);
        images.push(ExtractedImage {
            id: state.next_image_id(),
            relationship_id: Some(rel_id.to_string()),
            content_type,
            filename: path.rsplit('/').next().map(ToString::to_string),
            alt_text: alt_text.clone(),
            base64: if context.include_images {
                bytes.map(|bytes| general_purpose::STANDARD.encode(bytes))
            } else {
                None
            },
        });
    }

    Ok(images)
}

fn parse_table(
    table: Node<'_, '_>,
    _context: &DocxContext,
    forced_kind: Option<DocxBlockKind>,
) -> Result<DocxBlock, PipelineError> {
    let mut rows = Vec::new();
    for row in table
        .children()
        .filter(|node| is_element_named(*node, "tr"))
    {
        let mut cells = Vec::new();
        for cell in row.children().filter(|node| is_element_named(*node, "tc")) {
            let text = collect_text_content(cell).trim().to_string();
            let properties = cell.children().find(|node| is_element_named(*node, "tcPr"));
            let col_span = properties
                .and_then(|properties| find_child(properties, "gridSpan"))
                .and_then(|node| attr(node, W_NS, "val"))
                .and_then(|value| value.parse().ok());
            let mut metadata = BTreeMap::new();
            if let Some(value) = properties
                .and_then(|properties| find_child(properties, "vMerge"))
                .and_then(|node| attr(node, W_NS, "val").or(Some("continue")))
            {
                metadata.insert("v_merge".to_string(), serde_json::json!(value));
            }
            cells.push(TableCell {
                text,
                row_span: None,
                col_span,
                metadata,
            });
        }
        if !cells.is_empty() {
            rows.push(cells);
        }
    }
    let text = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| cell.text.as_str())
                .collect::<Vec<_>>()
                .join(" | ")
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(DocxBlock {
        kind: forced_kind.unwrap_or(DocxBlockKind::Table),
        text,
        level: None,
        table: Some(rows),
        image: None,
        list_format: None,
    })
}

fn paragraph_style_id<'a, 'input>(paragraph_props: Node<'a, 'input>) -> Option<&'a str> {
    find_child(paragraph_props, "pStyle").and_then(|node| attr(node, W_NS, "val"))
}

#[derive(Debug, Clone)]
struct ListInfo {
    level: u8,
    format: String,
}

fn paragraph_list_info(paragraph_props: Node<'_, '_>, context: &DocxContext) -> Option<ListInfo> {
    let num_pr = find_child(paragraph_props, "numPr")?;
    let num_id = find_child(num_pr, "numId")
        .and_then(|node| attr(node, W_NS, "val"))?
        .to_string();
    let level = find_child(num_pr, "ilvl")
        .and_then(|node| attr(node, W_NS, "val"))
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let format = context
        .numbering
        .get(&(num_id.clone(), level.to_string()))
        .cloned()
        .unwrap_or_else(|| "unknown".to_string());
    Some(ListInfo { level, format })
}

fn append_note_blocks(
    blocks: &mut Vec<DocxBlock>,
    kind: DocxBlockKind,
    refs: &[String],
    notes: &HashMap<String, String>,
) {
    for id in refs {
        let Some(text) = notes.get(id).filter(|text| !text.trim().is_empty()) else {
            continue;
        };
        blocks.push(DocxBlock {
            kind,
            text: text.clone(),
            level: None,
            table: None,
            image: None,
            list_format: None,
        });
    }
}

fn page_break_block() -> DocxBlock {
    DocxBlock {
        kind: DocxBlockKind::PageBreak,
        text: String::new(),
        level: None,
        table: None,
        image: None,
        list_format: None,
    }
}

fn leading_page_break_count(paragraph: Node<'_, '_>) -> usize {
    let mut count = 0usize;
    for descendant in paragraph.descendants().filter(|node| node.is_element()) {
        if is_page_break_node(descendant) {
            count += 1;
            continue;
        }
        if is_text_or_image_node(descendant) {
            break;
        }
    }
    count
}

fn is_page_break_node(node: Node<'_, '_>) -> bool {
    match node.tag_name().name() {
        "lastRenderedPageBreak" => true,
        "br" | "cr" => attr(node, W_NS, "type") == Some("page"),
        _ => false,
    }
}

fn is_text_or_image_node(node: Node<'_, '_>) -> bool {
    match node.tag_name().name() {
        "t" => node.text().is_some_and(|text| !text.trim().is_empty()),
        "drawing" | "pict" => true,
        _ => false,
    }
}

fn split_oversized_blocks(blocks: Vec<DocxBlock>, chunk_size: usize) -> Vec<DocxBlock> {
    let mut split = Vec::new();
    for block in blocks {
        if !matches!(
            block.kind,
            DocxBlockKind::Paragraph | DocxBlockKind::Header | DocxBlockKind::Footer
        ) || block.text.chars().count() <= chunk_size
        {
            split.push(block);
            continue;
        }

        for text in split_recursive(&block.text, chunk_size, 0) {
            split.push(DocxBlock {
                text,
                ..block.clone()
            });
        }
    }
    split
}

fn chunks_from_page_breaks(blocks: Vec<DocxBlock>) -> Option<(Vec<Vec<DocxBlock>>, String)> {
    let has_page_breaks = blocks
        .iter()
        .any(|block| matches!(block.kind, DocxBlockKind::PageBreak));
    if !has_page_breaks {
        return None;
    }

    let mut chunks = Vec::new();
    let mut current = Vec::new();
    for block in blocks {
        if matches!(block.kind, DocxBlockKind::PageBreak) {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(block);
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    (!chunks.is_empty()).then_some((chunks, "docx-page-breaks".to_string()))
}

fn chunk_blocks(blocks: Vec<DocxBlock>, chunk_size: usize) -> Vec<Vec<DocxBlock>> {
    if blocks.is_empty() {
        return vec![Vec::new()];
    }

    let mut chunks = Vec::new();
    let mut current: Vec<DocxBlock> = Vec::new();
    let mut current_len = 0usize;

    for block in blocks {
        let block_len = block_text_for_output(&block).chars().count();
        let current_has_body_content = current
            .iter()
            .any(|block| !matches!(block.kind, DocxBlockKind::Header | DocxBlockKind::Footer));
        let starts_new_section = matches!(block.kind, DocxBlockKind::Heading)
            && block.level.unwrap_or(9) <= 1
            && current_has_body_content;
        let exceeds_chunk = current_len > 0 && current_len + block_len > chunk_size;
        if starts_new_section || exceeds_chunk {
            chunks.push(std::mem::take(&mut current));
            current_len = 0;
        }
        current_len += block_len;
        current.push(block);
    }

    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn chunk_to_page(
    page_number: usize,
    filename: String,
    blocks: Vec<DocxBlock>,
    config: &PipelineConfig,
) -> PageOutput {
    let tables = if config.skip_tables || config.text_only {
        Vec::new()
    } else {
        blocks_to_tables(&blocks)
    };
    let mut embedded_images = blocks_to_images(&blocks);
    let image_base64 = if config.skip_images || config.text_only {
        None
    } else {
        first_png_image_base64(&blocks)
    };
    if !config.keep_base64_images {
        for image in &mut embedded_images {
            image.base64 = None;
        }
    }
    PageOutput {
        chunk_id: format!("chunk_{page_number}"),
        doc_title: filename,
        page_number: Some(page_number),
        text: blocks_to_text(&blocks),
        tables,
        extraction_warnings: Vec::new(),
        html: None,
        embedded_images,
        image_base64,
        image_text: None,
        image_classifier: None,
        image_text_1: None,
        image_text_2: None,
        image_text_3: None,
        summary_notes: None,
        summary_topics: None,
        summary_relevancy: None,
        summary_quality_validated: None,
        summary_notes_1: None,
        summary_notes_2: None,
        summary_notes_3: None,
        summary_budget_exhausted: None,
    }
}

fn blocks_to_text(blocks: &[DocxBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| {
            let text = block_text_for_output(block);
            if text.trim().is_empty() {
                None
            } else {
                Some(text)
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn block_text_for_output(block: &DocxBlock) -> String {
    match block.kind {
        DocxBlockKind::Heading => {
            let level = block.level.unwrap_or(1).clamp(1, 6) as usize;
            format!("{} {}", "#".repeat(level), block.text.trim())
        }
        DocxBlockKind::ListItem => {
            let indent = "  ".repeat(block.level.unwrap_or(1).saturating_sub(1) as usize);
            let marker = if block.list_format.as_deref() == Some("decimal") {
                "1."
            } else {
                "-"
            };
            format!("{indent}{marker} {}", block.text.trim())
        }
        DocxBlockKind::Table => block.text.trim().to_string(),
        DocxBlockKind::Image => format!(
            "[Image: {}]",
            block
                .image
                .as_ref()
                .and_then(|image| image.alt_text.as_deref().or(image.filename.as_deref()))
                .unwrap_or(block.text.as_str())
        ),
        DocxBlockKind::Footnote => format!("[Footnote] {}", block.text.trim()),
        DocxBlockKind::Endnote => format!("[Endnote] {}", block.text.trim()),
        DocxBlockKind::Comment => format!("[Comment] {}", block.text.trim()),
        DocxBlockKind::Header => format!("[Header] {}", block.text.trim()),
        DocxBlockKind::Footer => format!("[Footer] {}", block.text.trim()),
        DocxBlockKind::PageBreak => "[Page break]".to_string(),
        DocxBlockKind::SectionBreak => "[Section break]".to_string(),
        DocxBlockKind::Paragraph => block.text.trim().to_string(),
    }
}

fn blocks_to_tables(blocks: &[DocxBlock]) -> Vec<Vec<Vec<String>>> {
    blocks
        .iter()
        .filter_map(|block| {
            if !matches!(block.kind, DocxBlockKind::Table) {
                return None;
            }
            block.table.as_ref().map(|rows| {
                rows.iter()
                    .map(|row| row.iter().map(|cell| cell.text.clone()).collect())
                    .collect()
            })
        })
        .collect()
}

fn blocks_to_images(blocks: &[DocxBlock]) -> Vec<ExtractedImage> {
    blocks
        .iter()
        .filter_map(|block| block.image.clone())
        .collect()
}

fn first_png_image_base64(blocks: &[DocxBlock]) -> Option<String> {
    blocks.iter().find_map(|block| {
        let image = block.image.as_ref()?;
        let raw = image.base64.as_ref()?;
        let bytes = general_purpose::STANDARD.decode(raw).ok()?;
        opc::image_bytes_to_png_base64(&bytes, "DOCX image").ok()
    })
}

fn parse_styles(xml: &str) -> Result<HashMap<String, StyleInfo>, PipelineError> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|err| PipelineError::Extraction(format!("Invalid DOCX styles XML: {err}")))?;
    let mut styles = HashMap::new();
    for style in document
        .descendants()
        .filter(|node| is_element_named(*node, "style"))
    {
        let Some(style_id) = attr(style, W_NS, "styleId") else {
            continue;
        };
        let name = style
            .children()
            .find(|node| is_element_named(*node, "name"))
            .and_then(|node| attr(node, W_NS, "val"))
            .map(ToString::to_string);
        let heading_level =
            heading_level(style_id).or_else(|| name.as_deref().and_then(heading_level));
        styles.insert(style_id.to_string(), StyleInfo { heading_level });
    }
    Ok(styles)
}

fn heading_level(value: &str) -> Option<u8> {
    let normalized = value.to_ascii_lowercase().replace(' ', "");
    let suffix = normalized.strip_prefix("heading")?;
    suffix
        .parse::<u8>()
        .ok()
        .filter(|level| (1..=9).contains(level))
}

fn parse_numbering(xml: &str) -> Result<HashMap<(String, String), String>, PipelineError> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|err| PipelineError::Extraction(format!("Invalid DOCX numbering XML: {err}")))?;
    let mut abstract_map = HashMap::new();
    for abstract_num in document
        .descendants()
        .filter(|node| is_element_named(*node, "abstractNum"))
    {
        let Some(abstract_id) = attr(abstract_num, W_NS, "abstractNumId") else {
            continue;
        };
        for level in abstract_num
            .children()
            .filter(|node| is_element_named(*node, "lvl"))
        {
            let Some(ilvl) = attr(level, W_NS, "ilvl") else {
                continue;
            };
            let format = level
                .children()
                .find(|node| is_element_named(*node, "numFmt"))
                .and_then(|node| attr(node, W_NS, "val"))
                .unwrap_or("unknown");
            abstract_map.insert(
                (abstract_id.to_string(), ilvl.to_string()),
                format.to_string(),
            );
        }
    }

    let mut numbering = HashMap::new();
    for num in document
        .descendants()
        .filter(|node| is_element_named(*node, "num"))
    {
        let Some(num_id) = attr(num, W_NS, "numId") else {
            continue;
        };
        let Some(abstract_id) = num
            .children()
            .find(|node| is_element_named(*node, "abstractNumId"))
            .and_then(|node| attr(node, W_NS, "val"))
        else {
            continue;
        };
        for ((candidate_id, level), format) in &abstract_map {
            if candidate_id == abstract_id {
                numbering.insert((num_id.to_string(), level.clone()), format.clone());
            }
        }
    }
    Ok(numbering)
}

fn parse_notes(xml: &str, element_name: &str) -> Result<HashMap<String, String>, PipelineError> {
    let document = roxmltree::Document::parse(xml).map_err(|err| {
        PipelineError::Extraction(format!("Invalid DOCX {element_name} XML: {err}"))
    })?;
    let mut notes = HashMap::new();
    for note in document
        .descendants()
        .filter(|node| is_element_named(*node, element_name))
    {
        let Some(id) = attr(note, W_NS, "id") else {
            continue;
        };
        let text = collect_text_content(note).trim().to_string();
        if !text.is_empty() {
            notes.insert(id.to_string(), text);
        }
    }
    Ok(notes)
}

fn parse_content_types(xml: &str) -> Result<HashMap<String, String>, PipelineError> {
    let document = roxmltree::Document::parse(xml).map_err(|err| {
        PipelineError::Extraction(format!("Invalid DOCX content types XML: {err}"))
    })?;
    let mut types = HashMap::new();
    for default in document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "Default")
    {
        let Some(extension) = default.attribute("Extension") else {
            continue;
        };
        let Some(content_type) = default.attribute("ContentType") else {
            continue;
        };
        types.insert(extension.to_ascii_lowercase(), content_type.to_string());
    }
    Ok(types)
}

fn read_media_entries(
    archive: &mut SizeLimitedZipArchive,
    entry_names: &[String],
) -> Result<HashMap<String, Vec<u8>>, PipelineError> {
    let mut media = HashMap::new();
    for path in entry_names
        .iter()
        .filter(|name| name.starts_with("word/media/"))
    {
        media.insert(path.clone(), read_zip_bytes(archive, path)?);
    }
    Ok(media)
}

fn collect_text_content(node: Node<'_, '_>) -> String {
    let paragraphs = node
        .descendants()
        .filter(|descendant| is_element_named(*descendant, "p"))
        .filter_map(|paragraph| {
            let text = collect_inline_text(paragraph).trim().to_string();
            (!text.is_empty()).then_some(text)
        })
        .collect::<Vec<_>>();
    if !paragraphs.is_empty() {
        return paragraphs.join("\n");
    }

    collect_inline_text(node)
}

fn collect_inline_text(node: Node<'_, '_>) -> String {
    let mut text = String::new();
    for descendant in node.descendants().filter(|node| node.is_element()) {
        match descendant.tag_name().name() {
            "t" => {
                if let Some(value) = descendant.text() {
                    text.push_str(value);
                }
            }
            "tab" => text.push('\t'),
            "br" | "cr" => text.push('\n'),
            _ => {}
        }
    }
    text
}

fn content_type_for_path(path: &str, content_types: &HashMap<String, String>) -> Option<String> {
    path.rsplit_once('.')
        .and_then(|(_, extension)| content_types.get(&extension.to_ascii_lowercase()))
        .cloned()
}

fn zip_entry_names(archive: &mut SizeLimitedZipArchive) -> Vec<String> {
    (0..archive.len())
        .filter_map(|index| archive.entry_name(index))
        .collect()
}

fn read_zip_string(
    archive: &mut SizeLimitedZipArchive,
    path: &str,
) -> Result<String, PipelineError> {
    archive.read_string(path, "DOCX")
}

fn read_optional_zip_string(
    archive: &mut SizeLimitedZipArchive,
    path: &str,
) -> Result<Option<String>, PipelineError> {
    archive.read_optional_string(path, "DOCX")
}

fn read_zip_bytes(
    archive: &mut SizeLimitedZipArchive,
    path: &str,
) -> Result<Vec<u8>, PipelineError> {
    archive.read_bytes(path, "DOCX")
}

fn attr<'a, 'input>(node: Node<'a, 'input>, namespace: &str, name: &str) -> Option<&'a str> {
    node.attribute((namespace, name))
        .or_else(|| node.attribute(name))
}

fn find_child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|child| is_element_named(*child, name))
}

fn is_element_named(node: Node<'_, '_>, name: &str) -> bool {
    node.is_element() && node.tag_name().name() == name
}
