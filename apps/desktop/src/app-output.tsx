import {
  CheckCircle2,
  ChevronRight,
  FileJson,
  FilePlus2,
} from "lucide-react";
import { FieldBlock } from "./app-common";
import {
  formatDuration,
  formatMetricProvider,
  imageSrc,
  labelize,
  notesText,
} from "./app-core";
import { DesktopJob, DocumentOutput, PageOutput } from "./types";

export function OutputViewer({
  job,
  onExport,
  onProcessAnother,
  showCompletionActions = true,
}: {
  job: DesktopJob;
  onExport: (job: DesktopJob, kind: "markdown" | "json") => void;
  onProcessAnother: () => void;
  showCompletionActions?: boolean;
}) {
  const output = job.output as DocumentOutput;
  return (
    <div className="output-viewer progressive-output">
      <section className="completion-card">
        <div className="completion-header">
          <span className="completion-icon">
            <CheckCircle2 size={24} aria-hidden="true" />
          </span>
          <div>
            <h3>{output.document.filename}</h3>
            <p>
              {output.document.total_pages} pages or chunks processed in{" "}
              {formatDuration(job.duration_ms)}
            </p>
          </div>
        </div>
        {showCompletionActions && (
          <div className="completion-actions">
            <button
              className="button secondary compact"
              onClick={() => onExport(job, "json")}
            >
              <FileJson size={15} />
              Save JSON
            </button>
            <button
              className="button secondary compact completion-process-button"
              onClick={onProcessAnother}
            >
              <FilePlus2 size={15} />
              Process Another
            </button>
          </div>
        )}
      </section>

      <details className="disclosure-card">
        <summary>
          <span>Processing Metrics</span>
          <small>
            Document ID, providers, duration, token counts, and stage details
          </small>
        </summary>
        <MetricsView output={output} />
      </details>

      <details className="disclosure-card">
        <summary>
          <span>Page Details</span>
          <small>
            Extracted text, tables, vision output, and detailed attempts
          </small>
        </summary>
        <div className="page-list flat">
          {output.pages.map((page, index) => (
            <PageCard key={page.chunk_id} page={page} index={index} />
          ))}
        </div>
      </details>
    </div>
  );
}

function MetricsView({ output }: { output: DocumentOutput }) {
  const metrics = output.metrics;
  const visionProvider =
    metrics?.config.vision_extractor_provider ?? metrics?.config.vision_mode;
  const classifierProvider = metrics?.config.vision_classifier_provider;
  return (
    <section className="metrics">
      <div>
        <span className="metric-label">Document ID</span>
        <strong>{output.document.document_id}</strong>
      </div>
      <div>
        <span className="metric-label">Pages / chunks</span>
        <strong>{output.document.total_pages}</strong>
      </div>
      {metrics && (
        <>
          <div className="metric-card">
            <span>Total duration</span>
            <strong>{formatDuration(metrics.total_duration_ms)}</strong>
          </div>
          <div className="metric-card">
            <span>Total tokens</span>
            <strong>{metrics.total_tokens}</strong>
          </div>
          <div className="metric-card">
            <span>Vision provider</span>
            <strong>{formatMetricProvider(visionProvider)}</strong>
            {classifierProvider && classifierProvider !== visionProvider && (
              <small>
                Classifier: {formatMetricProvider(classifierProvider)}
              </small>
            )}
          </div>
          <div className="metric-card">
            <span>Summarizer provider</span>
            <strong>
              {formatMetricProvider(metrics.config.summarizer_provider)}
            </strong>
          </div>
          {Object.entries(metrics.stages).map(([stage, data]) => (
            <div className="metric-card" key={stage}>
              <span>{labelize(stage)}</span>
              <strong>{formatDuration(data.duration_ms)}</strong>
              <small>
                {data.pages_processed} pages, {data.tokens} tokens
                {data.avg_relevancy != null
                  ? `, ${data.avg_relevancy}% avg relevancy`
                  : ""}
                {data.total_attempts != null
                  ? `, ${data.total_attempts} attempts`
                  : ""}
                {data.pages_with_images != null
                  ? `, ${data.pages_with_images} pages with images`
                  : ""}
                {data.classified_count != null
                  ? `, ${data.classified_count} classified`
                  : ""}
                {data.extracted_count != null
                  ? `, ${data.extracted_count} extracted`
                  : ""}
              </small>
            </div>
          ))}
        </>
      )}
    </section>
  );
}

function PageCard({ page, index }: { page: PageOutput; index: number }) {
  const embeddedImages = page.embedded_images ?? [];
  const summaryUnvalidated =
    page.summary_quality_validated === false && !!page.summary_notes?.length;

  return (
    <details className="page-card">
      <summary className="page-card-header">
        <div>
          <strong>Page {page.page_number ?? index + 1}</strong>
          <span>
            {page.chunk_id} / {page.doc_title}
          </span>
        </div>
        <span className="page-card-status">
          {page.summary_budget_exhausted && (
            <mark>{page.summary_budget_exhausted}</mark>
          )}
          {summaryUnvalidated && <mark>unvalidated</mark>}
          {page.summary_relevancy != null && (
            <mark>{page.summary_relevancy}%</mark>
          )}
          <ChevronRight
            className="page-card-chevron"
            size={18}
            aria-hidden="true"
          />
        </span>
      </summary>
      <div className="page-content">
        <FieldBlock
          title="Warnings"
          empty={
            !page.extraction_warnings?.length &&
            !page.summary_budget_exhausted &&
            !summaryUnvalidated
          }
        >
          <ul>
            {(page.extraction_warnings ?? []).map((warning) => (
              <li key={warning}>{labelize(warning)}</li>
            ))}
            {page.summary_budget_exhausted && (
              <li>
                Summary budget exhausted:{" "}
                {labelize(page.summary_budget_exhausted)}
              </li>
            )}
            {summaryUnvalidated && (
              <li>Summary quality validation not reached.</li>
            )}
          </ul>
        </FieldBlock>

        <FieldBlock title="Text" empty={!page.text.trim()}>
          <pre>{page.text}</pre>
        </FieldBlock>

        <FieldBlock title="Topics" empty={!page.summary_topics?.length}>
          <div className="topic-list">
            {(page.summary_topics ?? []).map((topic) => (
              <span key={topic}>{topic}</span>
            ))}
          </div>
        </FieldBlock>

        <FieldBlock title="Summary Notes" empty={!page.summary_notes?.length}>
          <ul>
            {(page.summary_notes ?? []).map((note, i) => (
              <li key={i}>{note}</li>
            ))}
          </ul>
        </FieldBlock>

        <FieldBlock title="Tables" empty={!page.tables.length}>
          <div className="table-stack">
            {page.tables.map((table, tableIndex) => (
              <div className="table-wrap" key={tableIndex}>
                <table>
                  <tbody>
                    {table.map((row, rowIndex) => (
                      <tr key={rowIndex}>
                        {row.map((cell, cellIndex) => (
                          <td key={cellIndex}>{cell}</td>
                        ))}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            ))}
          </div>
        </FieldBlock>

        <FieldBlock title="Embedded Images" empty={!embeddedImages.length}>
          <div className="embedded-image-list">
            {embeddedImages.map((image) => (
              <figure className="embedded-image-card" key={image.id}>
                {image.base64 && (
                  <img
                    src={embeddedImageSrc(image)}
                    alt={image.alt_text ?? image.filename ?? image.id}
                  />
                )}
                <figcaption>
                  <strong>
                    {image.alt_text ?? image.filename ?? image.id}
                  </strong>
                  {image.content_type && <span>{image.content_type}</span>}
                </figcaption>
              </figure>
            ))}
          </div>
        </FieldBlock>

        <FieldBlock
          title="Image Text"
          empty={!page.image_text && page.image_classifier == null}
        >
          <p>
            <strong>Classifier:</strong>{" "}
            {page.image_classifier == null
              ? "not run"
              : page.image_classifier
                ? "contains visuals"
                : "no visuals"}
          </p>
          {page.image_text && <pre>{page.image_text}</pre>}
          <AttemptList
            title="Vision attempts"
            values={[page.image_text_1, page.image_text_2, page.image_text_3]}
          />
        </FieldBlock>

        {page.image_base64 && (
          <figure className="page-image">
            <img
              src={imageSrc(page.image_base64)}
              alt={`Rendered page ${page.page_number ?? index + 1}`}
            />
          </figure>
        )}

        <FieldBlock
          title="Detailed Summary Attempts"
          empty={
            !page.summary_notes_1 &&
            !page.summary_notes_2 &&
            !page.summary_notes_3
          }
        >
          <AttemptList
            title="Summary attempts"
            values={[
              notesText(page.summary_notes_1),
              notesText(page.summary_notes_2),
              notesText(page.summary_notes_3),
            ]}
          />
        </FieldBlock>
      </div>
    </details>
  );
}

function embeddedImageSrc(
  image: NonNullable<PageOutput["embedded_images"]>[number],
): string {
  const contentType = image.content_type || "image/png";
  return `data:${contentType};base64,${image.base64 ?? ""}`;
}

function AttemptList({
  title,
  values,
}: {
  title: string;
  values: Array<string | null | undefined>;
}) {
  const populated = values.filter((value): value is string => !!value?.trim());
  if (!populated.length) return null;
  return (
    <div className="attempt-list">
      <strong>{title}</strong>
      {populated.map((value, index) => (
        <pre key={index}>{value}</pre>
      ))}
    </div>
  );
}
