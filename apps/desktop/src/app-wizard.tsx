import {
  Check,
  CheckCircle2,
  ChevronLeft,
  ChevronRight,
  Eye,
  FileText,
  ListOrdered,
  RotateCcw,
  SlidersHorizontal,
} from "lucide-react";
import * as React from "react";
import {
  ChoiceGroup,
  NumberField,
  ReviewCard,
  SelectField,
  SwitchRow,
} from "./app-common";
import {
  buildPipelineSteps,
  DPI_OPTIONS,
  dpiLabel,
  FileKind,
  firstVisibleVisionProvider,
  normalizedConfigForProviderVisibility,
  normalizedSummarizerValue,
  normalizedVisionValue,
  PipelineStepId,
  ProviderAvailabilityMap,
  SUMMARIZER_MODE_OPTIONS,
  summarizerModeLabel,
  summarizerProviderLabel,
  visibleSummarizerProviderOptions,
  visibleVisionProviderOptions,
  visionLabel,
} from "./app-core";
import {
  PdfImageDpi,
  PipelineConfig,
  ProviderVisibilitySettings,
  VisionMode,
} from "./types";

export function PipelineWizard({
  fileKind,
  fileCount,
  config,
  providerVisibility,
  providerAvailability,
  onConfigChange,
  onReset,
  onStart,
}: {
  fileKind: FileKind;
  fileCount: number;
  config: PipelineConfig;
  providerVisibility: ProviderVisibilitySettings;
  providerAvailability: ProviderAvailabilityMap;
  onConfigChange: (config: PipelineConfig) => void;
  onReset: () => void;
  onStart: () => void;
}) {
  const [currentStepIndex, setCurrentStepIndex] = React.useState(0);
  const [advancedExtraction, setAdvancedExtraction] = React.useState(false);
  const [advancedVision, setAdvancedVision] = React.useState(false);
  const steps = React.useMemo(
    () => buildPipelineSteps(fileKind, config.extract_only),
    [fileKind, config.extract_only],
  );
  const safeStepIndex = Math.min(currentStepIndex, steps.length - 1);
  const currentStep = steps[safeStepIndex];
  const isFirstStep = safeStepIndex === 0;
  const isLastStep = safeStepIndex === steps.length - 1;

  React.useEffect(() => {
    if (safeStepIndex !== currentStepIndex) setCurrentStepIndex(safeStepIndex);
  }, [currentStepIndex, safeStepIndex]);

  React.useEffect(() => {
    const nextConfig = normalizedConfigForProviderVisibility(
      config,
      providerVisibility,
    );
    if (nextConfig) onConfigChange(nextConfig);
  }, [config, onConfigChange, providerVisibility]);

  const update = <K extends keyof PipelineConfig>(
    key: K,
    value: PipelineConfig[K],
  ) => {
    onConfigChange({ ...config, [key]: value });
  };
  const patchConfig = (patch: Partial<PipelineConfig>) => {
    onConfigChange({ ...config, ...patch });
  };

  return (
    <section className="wizard-card">
      <div className="wizard-header">
        <div>
          <h3>Configure Pipeline</h3>
          <p>Setup processing options in {steps.length} steps.</p>
        </div>
        <button className="button ghost" type="button" onClick={onReset}>
          <RotateCcw size={16} />
          Reset
        </button>
      </div>

      <StepIndicator steps={steps} currentStepIndex={safeStepIndex} />

      <div className="wizard-step-body">
        {currentStep.id === "extraction" && (
          <ExtractionStep
            config={config}
            fileKind={fileKind}
            advanced={advancedExtraction}
            onAdvancedChange={setAdvancedExtraction}
            onUpdate={update}
          />
        )}
        {currentStep.id === "vision" && (
          <VisionStep
            config={config}
            providerVisibility={providerVisibility}
            providerAvailability={providerAvailability}
            advanced={advancedVision}
            onAdvancedChange={setAdvancedVision}
            onUpdate={update}
            onPatch={patchConfig}
          />
        )}
        {currentStep.id === "chunking" && (
          <ChunkingStep config={config} onUpdate={update} />
        )}
        {currentStep.id === "summarization" && (
          <SummarizationStep
            config={config}
            fileKind={fileKind}
            providerVisibility={providerVisibility}
            providerAvailability={providerAvailability}
            onUpdate={update}
          />
        )}
        {currentStep.id === "review" && (
          <ReviewStep config={config} fileKind={fileKind} />
        )}
      </div>

      <div className="wizard-actions">
        <button
          className="button secondary"
          type="button"
          onClick={() => setCurrentStepIndex((step) => step - 1)}
          disabled={isFirstStep}
        >
          <ChevronLeft size={16} />
          Back
        </button>
        {!isLastStep ? (
          <button
            className="button primary"
            type="button"
            onClick={() => setCurrentStepIndex((step) => step + 1)}
          >
            Next
            <ChevronRight size={16} />
          </button>
        ) : (
          <button className="button primary" type="button" onClick={onStart}>
            <ListOrdered size={16} />
            Add {fileCount} to Queue
          </button>
        )}
      </div>
    </section>
  );
}

function StepIndicator({
  steps,
  currentStepIndex,
}: {
  steps: Array<{ id: PipelineStepId; title: string; description: string }>;
  currentStepIndex: number;
}) {
  return (
    <div className="stepper" aria-label="Pipeline steps" role="list">
      {steps.map((step, index) => {
        const isCurrent = currentStepIndex === index;
        const isDone = currentStepIndex > index;
        return (
          <React.Fragment key={step.id}>
            <div
              className={`stepper-item ${isCurrent ? "current" : ""} ${isDone ? "done" : ""}`}
              role="listitem"
              aria-current={isCurrent ? "step" : undefined}
            >
              <span className="stepper-dot" aria-hidden="true">
                {isDone ? <Check size={16} aria-hidden="true" /> : index + 1}
              </span>
              <span>{step.title}</span>
            </div>
            {index < steps.length - 1 && (
              <span
                className={`stepper-line ${isDone ? "done" : ""}`}
                aria-hidden="true"
              />
            )}
          </React.Fragment>
        );
      })}
    </div>
  );
}

function ExtractionStep({
  config,
  fileKind,
  advanced,
  onAdvancedChange,
  onUpdate,
}: {
  config: PipelineConfig;
  fileKind: FileKind;
  advanced: boolean;
  onAdvancedChange: (value: boolean) => void;
  onUpdate: <K extends keyof PipelineConfig>(
    key: K,
    value: PipelineConfig[K],
  ) => void;
}) {
  const isPptx = fileKind === "pptx";
  const isDocx = fileKind === "docx";
  const isText = fileKind === "text";
  const isMixed = fileKind === "mixed";

  if (isText) return null;

  return (
    <div className="step-stack">
      <SwitchRow
        label="Extraction"
        description={`Extract text${
          isPptx
            ? ", notes, and slide structure"
            : isDocx
              ? ", tables, notes, and document structure"
              : isMixed
                ? ", tables, slides, and document structure where available"
                : ", tables, and document structure"
        } before AI processing`}
        checked={config.run_extraction}
        onChange={(checked) => {
          onUpdate("run_extraction", checked);
          if (!checked) {
            onUpdate("extract_only", false);
            onUpdate("text_only", false);
            onUpdate("skip_images", false);
            onUpdate("vision_skip_classification", true);
          }
        }}
      />

      {config.run_extraction ? (
        <>
          <SwitchRow
            label="Advanced Mode"
            description="Fine-tune parsing and output size"
            checked={advanced}
            onChange={onAdvancedChange}
          />
          {advanced && (
            <div className="advanced-card">
              <SwitchRow
                label="Extract Only"
                description="Stop after extraction and skip vision and summarization"
                checked={config.extract_only}
                onChange={(checked) => onUpdate("extract_only", checked)}
              />
              <SwitchRow
                label="Text Only"
                description={
                  isPptx
                    ? "Extract only slide text"
                    : "Extract only text and skip tables and images"
                }
                checked={config.text_only}
                onChange={(checked) => onUpdate("text_only", checked)}
              />
              <SwitchRow
                label={isPptx ? "Skip Slide Notes" : "Skip Tables"}
                description={
                  isPptx
                    ? "Do not extract speaker notes"
                    : `Do not extract tables from ${isDocx ? "Word documents" : isMixed ? "supported documents" : "PDFs"}`
                }
                checked={config.skip_tables}
                onChange={(checked) => onUpdate("skip_tables", checked)}
              />
              {isPptx && (
                <SwitchRow
                  label="Skip Slide Tables"
                  description="Do not extract tables from slides"
                  checked={config.skip_pptx_tables}
                  onChange={(checked) => onUpdate("skip_pptx_tables", checked)}
                />
              )}
              <SwitchRow
                label={isPptx ? "Skip Slide Screenshots" : "Skip Images"}
                description={
                  isPptx
                    ? "Do not render slide screenshots for vision"
                    : "Do not extract images from the document"
                }
                checked={config.skip_images}
                onChange={(checked) => onUpdate("skip_images", checked)}
              />
              <SwitchRow
                label="Keep Base64 Images"
                description="Include image payloads in the output JSON"
                checked={config.keep_base64_images}
                onChange={(checked) => onUpdate("keep_base64_images", checked)}
              />
            </div>
          )}
        </>
      ) : (
        <div className="soft-note">
          Vision can still render page images when extraction is disabled.
        </div>
      )}
    </div>
  );
}

function VisionStep({
  config,
  providerVisibility,
  providerAvailability,
  advanced,
  onAdvancedChange,
  onUpdate,
  onPatch,
}: {
  config: PipelineConfig;
  providerVisibility: ProviderVisibilitySettings;
  providerAvailability: ProviderAvailabilityMap;
  advanced: boolean;
  onAdvancedChange: (value: boolean) => void;
  onUpdate: <K extends keyof PipelineConfig>(
    key: K,
    value: PipelineConfig[K],
  ) => void;
  onPatch: (patch: Partial<PipelineConfig>) => void;
}) {
  const visionEnabled = config.vision_mode !== "none";
  const visionOptions = visibleVisionProviderOptions(providerVisibility.vision);
  const classifierOptions = visibleVisionProviderOptions(
    providerVisibility.classifier,
  );
  const firstVision = firstVisibleVisionProvider(providerVisibility.vision);
  const firstClassifier = firstVisibleVisionProvider(
    providerVisibility.classifier,
  );

  return (
    <div className="step-stack">
      <SwitchRow
        label="Vision Processing"
        description="Analyze images and visual content in rendered pages"
        checked={visionEnabled}
        onChange={(checked) => {
          if (checked) {
            onPatch({ vision_mode: firstVision });
          } else {
            onAdvancedChange(false);
            onPatch({
              vision_mode: "none",
              vision_classifier_mode: undefined,
              vision_extractor_mode: undefined,
              vision_skip_classification: false,
            });
          }
        }}
      />

      {visionEnabled && (
        <>
          <div className="split-row">
            <SwitchRow
              label="Advanced Mode"
              description="Use separate classifier and extractor providers"
              checked={advanced}
              onChange={(checked) => {
                onAdvancedChange(checked);
                if (checked) {
                  onPatch({
                    vision_classifier_mode: config.vision_skip_classification
                      ? undefined
                      : normalizedVisionValue(
                          config.vision_classifier_mode ?? firstClassifier,
                          classifierOptions,
                        ),
                    vision_extractor_mode: normalizedVisionValue(
                      config.vision_extractor_mode ??
                        (config.vision_mode === "none"
                          ? firstVision
                          : config.vision_mode),
                      visionOptions,
                    ),
                  });
                } else {
                  onPatch({
                    vision_classifier_mode: undefined,
                    vision_extractor_mode: undefined,
                    vision_skip_classification: false,
                  });
                }
              }}
            />
            <SelectField
              label="PDF Image DPI"
              value={String(config.pdf_image_dpi)}
              options={DPI_OPTIONS.map(String)}
              onChange={(value) =>
                onUpdate("pdf_image_dpi", Number(value) as PdfImageDpi)
              }
            />
          </div>

          {!advanced ? (
            <ChoiceGroup
              label="Vision Provider"
              description="Choose a provider for image and visual content analysis"
              value={normalizedVisionValue(
                config.vision_mode === "none"
                  ? firstVision
                  : config.vision_mode,
                visionOptions,
              )}
              options={visionOptions}
              availability={providerAvailability}
              availabilityRole="vision"
              onChange={(value) => onUpdate("vision_mode", value)}
            />
          ) : (
            <>
              <SwitchRow
                label="Disable Classification"
                description="Run extraction on every rendered page"
                checked={config.vision_skip_classification}
                onChange={(checked) =>
                  onUpdate("vision_skip_classification", checked)
                }
              />
              {!config.vision_skip_classification && (
                <ChoiceGroup
                  label="Classification Provider"
                  description="Identify pages that contain visual content"
                  value={normalizedVisionValue(
                    config.vision_classifier_mode ?? firstClassifier,
                    classifierOptions,
                  )}
                  options={classifierOptions}
                  availability={providerAvailability}
                  availabilityRole="vision"
                  onChange={(value) =>
                    onUpdate("vision_classifier_mode", value)
                  }
                />
              )}
              <ChoiceGroup
                label="Extraction Provider"
                description="Extract detailed visual content from selected pages"
                value={normalizedVisionValue(
                  config.vision_extractor_mode ??
                    (config.vision_mode === "none"
                      ? firstVision
                      : config.vision_mode),
                  visionOptions,
                )}
                options={visionOptions}
                availability={providerAvailability}
                availabilityRole="vision"
                onChange={(value) => onUpdate("vision_extractor_mode", value)}
              />
            </>
          )}
        </>
      )}
    </div>
  );
}

function ChunkingStep({
  config,
  onUpdate,
}: {
  config: PipelineConfig;
  onUpdate: <K extends keyof PipelineConfig>(
    key: K,
    value: PipelineConfig[K],
  ) => void;
}) {
  return (
    <div className="step-stack">
      <div className="soft-note">
        Text documents are split into chunks before summarization.
      </div>
      <NumberField
        label="Chunk size"
        value={config.chunk_size}
        min={1000}
        step={100}
        onChange={(value) => onUpdate("chunk_size", value)}
      />
      <NumberField
        label="Chunk overlap"
        value={config.chunk_overlap}
        min={0}
        step={10}
        onChange={(value) => onUpdate("chunk_overlap", value)}
      />
    </div>
  );
}

function SummarizationStep({
  config,
  fileKind,
  providerVisibility,
  providerAvailability,
  onUpdate,
}: {
  config: PipelineConfig;
  fileKind: FileKind;
  providerVisibility: ProviderVisibilitySettings;
  providerAvailability: ProviderAvailabilityMap;
  onUpdate: <K extends keyof PipelineConfig>(
    key: K,
    value: PipelineConfig[K],
  ) => void;
}) {
  const summarizerOptions = visibleSummarizerProviderOptions(
    providerVisibility.summarizer,
  );

  return (
    <div className="step-stack">
      <SwitchRow
        label="Enable Summarization"
        description={`Generate summaries for ${fileKind === "text" ? "text chunks" : fileKind === "mixed" ? "text chunks and extracted pages" : "extracted pages"}`}
        checked={config.run_summarization}
        onChange={(checked) => onUpdate("run_summarization", checked)}
      />
      {config.run_summarization && (
        <>
          <ChoiceGroup
            label="AI Provider"
            description="Choose the LLM provider for summarization"
            value={normalizedSummarizerValue(
              config.summarizer_provider,
              summarizerOptions,
            )}
            options={summarizerOptions}
            availability={providerAvailability}
            availabilityRole="summarizer"
            onChange={(value) => onUpdate("summarizer_provider", value)}
          />
          <ChoiceGroup
            label="Summarization Mode"
            description="Choose the type of summary output"
            value={config.summarizer_mode}
            options={SUMMARIZER_MODE_OPTIONS}
            onChange={(value) => onUpdate("summarizer_mode", value)}
          />
          <SwitchRow
            label="Detailed Summarization"
            description="Run summarization multiple times and synthesize the result"
            checked={config.summarizer_detailed_extraction}
            onChange={(checked) =>
              onUpdate("summarizer_detailed_extraction", checked)
            }
          />
          <SwitchRow
            label="Insight Mode"
            description="Synthesize context first, then extract focused knowledge-base insights"
            checked={config.summarizer_insight_mode}
            onChange={(checked) => onUpdate("summarizer_insight_mode", checked)}
          />
          <div className="field-grid">
            <NumberField
              label="Max tokens/page"
              value={config.max_tokens_per_page}
              min={1000}
              step={1000}
              onChange={(value) => onUpdate("max_tokens_per_page", value)}
            />
            <NumberField
              label="Max seconds/page"
              value={config.max_seconds_per_page}
              min={5}
              step={5}
              onChange={(value) => onUpdate("max_seconds_per_page", value)}
            />
          </div>
        </>
      )}
    </div>
  );
}

function ReviewStep({
  config,
  fileKind,
}: {
  config: PipelineConfig;
  fileKind: FileKind;
}) {
  const extractionTitle =
    fileKind === "pptx"
      ? "PowerPoint Extraction"
      : fileKind === "docx"
        ? "Word Extraction"
        : fileKind === "mixed"
          ? "Document Extraction"
          : "PDF Extraction";
  const visionRows = buildVisionReviewRows(config, fileKind);

  return (
    <div className="review-stack">
      {fileKind !== "text" && (
        <ReviewCard
          icon={<FileText size={17} />}
          title={extractionTitle}
          rows={[
            ["Extraction", config.run_extraction],
            ["Extract Only", config.extract_only],
            ["Text Only", config.text_only],
            [
              fileKind === "pptx" ? "Skip Slide Notes" : "Skip Tables",
              config.skip_tables,
            ],
            ...(fileKind === "pptx"
              ? ([["Skip Slide Tables", config.skip_pptx_tables]] as Array<
                  [string, boolean | string]
                >)
              : []),
            [
              fileKind === "pptx" ? "Skip Slide Screenshots" : "Skip Images",
              config.skip_images,
            ],
            ["Keep Base64 Images", config.keep_base64_images],
          ]}
        />
      )}
      {(fileKind === "text" || fileKind === "mixed") && (
        <ReviewCard
          icon={<SlidersHorizontal size={17} />}
          title="Text Chunking"
          rows={[
            ["Chunk Size", String(config.chunk_size)],
            ["Chunk Overlap", String(config.chunk_overlap)],
          ]}
        />
      )}
      {!config.extract_only && fileKind !== "text" && (
        <ReviewCard
          icon={<Eye size={17} />}
          title="Vision Processing"
          rows={visionRows}
        />
      )}
      {!config.extract_only && (
        <ReviewCard
          icon={<CheckCircle2 size={17} />}
          title="Summarization"
          rows={[
            ["Enabled", config.run_summarization],
            [
              "Provider",
              config.run_summarization
                ? summarizerProviderLabel(config.summarizer_provider)
                : "Disabled",
            ],
            [
              "Mode",
              config.run_summarization
                ? summarizerModeLabel(config.summarizer_mode)
                : "Disabled",
            ],
            [
              "Max Tokens/Page",
              config.run_summarization
                ? String(config.max_tokens_per_page)
                : "Disabled",
            ],
            [
              "Max Seconds/Page",
              config.run_summarization
                ? String(config.max_seconds_per_page)
                : "Disabled",
            ],
          ]}
        />
      )}
      <div className="soft-note">
        Review these settings, then add the selected documents to the queue.
      </div>
    </div>
  );
}

function buildVisionReviewRows(
  config: PipelineConfig,
  fileKind: FileKind,
): Array<[string, boolean | string]> {
  if (config.vision_mode === "none") return [["Provider", "Disabled"]];

  const extractor = resolveReviewVisionMode(
    config.vision_extractor_mode ?? config.vision_mode,
    config.vision_cli_provider,
  );
  const classifier = resolveReviewVisionMode(
    config.vision_classifier_mode ?? config.vision_mode,
    config.vision_cli_provider,
  );
  const rows: Array<[string, boolean | string]> =
    !config.vision_skip_classification && classifier !== extractor
      ? [
          ["Extractor", visionLabel(extractor)],
          ["Classifier", visionLabel(classifier)],
        ]
      : [["Provider", visionLabel(extractor)]];

  if (config.vision_skip_classification) {
    rows.push(["Classification", "Disabled"]);
  }
  if (fileKind === "pdf") {
    rows.push(["PDF Image DPI", dpiLabel(config.pdf_image_dpi)]);
  }

  return rows;
}

function resolveReviewVisionMode(
  mode: VisionMode,
  cliProvider?: PipelineConfig["vision_cli_provider"],
): VisionMode {
  if (
    cliProvider &&
    (mode === "codex" || mode === "claude" || mode === "grok")
  ) {
    return cliProvider;
  }
  return mode;
}
