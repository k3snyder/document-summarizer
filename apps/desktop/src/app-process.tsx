import { Check, CircleStop, Files, FileText, Upload, X } from "lucide-react";
import * as React from "react";
import { StatusIcon } from "./app-common";
import {
  basename,
  buildProcessingStages,
  clampProgress,
  formatElapsedDuration,
  getBatchFileKind,
  getFileKind,
  orderQueuedJobs,
  progressUnitLabel,
  ProviderAvailabilityMap,
  queueJobSubtitle,
  useElapsedMilliseconds,
} from "./app-core";
import { OutputViewer } from "./app-output";
import { PipelineWizard } from "./app-wizard";
import {
  DEFAULT_PIPELINE_CONFIG,
  DesktopJob,
  DesktopJobProgress,
  PipelineConfig,
  ProviderVisibilitySettings,
} from "./types";

export function ProcessView({
  selectedFiles,
  selectedJob,
  selectedJobId,
  jobs,
  activeJob,
  activeProgress,
  config,
  providerVisibility,
  providerAvailability,
  isFileDragOver,
  onChooseFiles,
  onRemoveFile,
  onClearFiles,
  onEnqueue,
  onConfigChange,
  onSelectJob,
  onCancelJob,
  onExport,
  onProcessAnother,
}: {
  selectedFiles: string[];
  selectedJob: DesktopJob | null;
  selectedJobId: string | null;
  jobs: DesktopJob[];
  activeJob: DesktopJob | null;
  activeProgress: DesktopJobProgress | null;
  config: PipelineConfig;
  providerVisibility: ProviderVisibilitySettings;
  providerAvailability: ProviderAvailabilityMap;
  isFileDragOver: boolean;
  onChooseFiles: () => void;
  onRemoveFile: (filePath: string) => void;
  onClearFiles: () => void;
  onEnqueue: () => void;
  onConfigChange: (config: PipelineConfig) => void;
  onSelectJob: (jobId: string) => void;
  onCancelJob: (jobId: string) => void;
  onExport: (job: DesktopJob, kind: "markdown" | "json") => void;
  onProcessAnother: () => void;
}) {
  const queuedJobs = React.useMemo(() => orderQueuedJobs(jobs), [jobs]);

  return (
    <div className="pipeline-shell">
      {!selectedFiles.length && !activeJob && (
        <UploadPrompt
          isDragOver={isFileDragOver}
          onChooseFiles={onChooseFiles}
        />
      )}
      {!!selectedFiles.length && (
        <>
          <SelectedFilesCard
            filePaths={selectedFiles}
            isDragOver={isFileDragOver}
            onAdd={onChooseFiles}
            onRemove={onRemoveFile}
            onClear={onClearFiles}
          />
          <PipelineWizard
            fileKind={getBatchFileKind(selectedFiles)}
            fileCount={selectedFiles.length}
            config={config}
            providerVisibility={providerVisibility}
            providerAvailability={providerAvailability}
            onConfigChange={onConfigChange}
            onReset={() => onConfigChange({ ...DEFAULT_PIPELINE_CONFIG })}
            onStart={onEnqueue}
          />
        </>
      )}
      {activeJob && (
        <ProcessingProgressCard
          job={activeJob}
          config={activeJob.config ?? config}
          progress={activeProgress}
        />
      )}
      {!!queuedJobs.length && (
        <QueuePanel
          jobs={queuedJobs}
          selectedJobId={selectedJobId}
          onSelectJob={onSelectJob}
          onCancelJob={onCancelJob}
        />
      )}
      {selectedJob?.output && (
        <div className="process-output-section">
          <OutputViewer
            job={selectedJob}
            onExport={onExport}
            onProcessAnother={onProcessAnother}
          />
        </div>
      )}
    </div>
  );
}

function ProcessingProgressCard({
  job,
  config,
  progress,
}: {
  job: DesktopJob;
  config: PipelineConfig;
  progress: DesktopJobProgress | null;
}) {
  const stages = buildProcessingStages(config);
  const progressStage = progress?.stage ?? stages[0]?.id ?? "extraction";
  const currentStageIndex =
    progressStage === "completed"
      ? stages.length
      : stages.findIndex((stage) => stage.id === progressStage);
  const safeCurrentStageIndex = currentStageIndex >= 0 ? currentStageIndex : 0;
  const progressPercent = clampProgress(progress?.progress ?? 3);
  const unitLabel = progressUnitLabel(job.file_name);
  const pageLabel =
    progress?.page_number && progress.total_pages
      ? `${unitLabel} ${progress.page_number} of ${progress.total_pages}`
      : null;
  const elapsedMs = useElapsedMilliseconds(job.started_at, job.duration_ms);

  return (
    <section className="wizard-card pipeline-progress-card">
      <div className="progress-card-header">
        <div>
          <h3>Processing {job.file_name}</h3>
        </div>
        <div className="processing-status-group">
          <span className="elapsed-pill">
            Elapsed {formatElapsedDuration(elapsedMs)}
          </span>
        </div>
      </div>

      <div
        className="runtime-stepper"
        aria-label="Pipeline progress"
        role="list"
      >
        {stages.map((stage, index) => {
          const isDone =
            safeCurrentStageIndex > index || progressStage === "completed";
          const isCurrent =
            safeCurrentStageIndex === index && progressStage !== "completed";
          return (
            <React.Fragment key={stage.id}>
              <div
                className={`runtime-stage ${isDone ? "done" : ""} ${isCurrent ? "current" : ""}`}
                role="listitem"
                aria-current={isCurrent ? "step" : undefined}
              >
                <span className="runtime-stage-dot" aria-hidden="true">
                  {isDone ? (
                    <Check size={18} aria-hidden="true" />
                  ) : isCurrent ? (
                    <span className="spinner small" />
                  ) : (
                    index + 1
                  )}
                </span>
                <span>{stage.label}</span>
              </div>
              {index < stages.length - 1 && (
                <span
                  className={`runtime-stage-line ${isDone ? "done" : ""}`}
                  aria-hidden="true"
                />
              )}
            </React.Fragment>
          );
        })}
      </div>

      <div className="progress-meter-heading">
        <span>Overall Progress</span>
        <strong>{progressPercent}%</strong>
      </div>
      <div className="progress-meter" aria-label="Overall progress">
        <span style={{ width: `${progressPercent}%` }} />
      </div>

      <div className="progress-message-row">
        <p>{progress?.message ?? "Starting extraction."}</p>
        {pageLabel && <span>{pageLabel}</span>}
      </div>
    </section>
  );
}

function UploadPrompt({
  isDragOver,
  onChooseFiles,
}: {
  isDragOver: boolean;
  onChooseFiles: () => void;
}) {
  return (
    <button
      type="button"
      className={`upload-card ${isDragOver ? "drag-over" : ""}`}
      onClick={onChooseFiles}
    >
      <span className="upload-icon">
        <Upload size={30} aria-hidden="true" />
      </span>
      <strong>
        {isDragOver ? "Drop to add these documents" : "Drop documents here"}
      </strong>
      <span>or click to browse</span>
      <small>PDF, PowerPoint, Word, TXT, or Markdown files</small>
    </button>
  );
}

function SelectedFilesCard({
  filePaths,
  isDragOver,
  onAdd,
  onRemove,
  onClear,
}: {
  filePaths: string[];
  isDragOver: boolean;
  onAdd: () => void;
  onRemove: (filePath: string) => void;
  onClear: () => void;
}) {
  return (
    <section className={`selected-files-card ${isDragOver ? "drag-over" : ""}`}>
      <header className="selected-files-header">
        <div>
          <h3>
            {filePaths.length} selected document
            {filePaths.length === 1 ? "" : "s"}
          </h3>
          <p>One queue job will be created for each document.</p>
        </div>
        <div className="selected-files-actions">
          <button
            className="button secondary compact"
            type="button"
            onClick={onAdd}
          >
            <Files size={15} />
            Add
          </button>
          <button
            className="button ghost compact"
            type="button"
            onClick={onClear}
          >
            Clear
          </button>
        </div>
      </header>
      <div className="selected-file-list">
        {filePaths.map((filePath) => {
          const fileKind = getFileKind(filePath);
          return (
            <div className="selected-file-row" key={filePath}>
              <FileText
                className={`file-kind ${fileKind}`}
                size={30}
                aria-hidden="true"
              />
              <span>
                <strong>{basename(filePath)}</strong>
                <small>{filePath}</small>
              </span>
              <button
                className="icon-button"
                type="button"
                onClick={() => onRemove(filePath)}
                aria-label={`Remove ${basename(filePath)}`}
              >
                <X size={18} />
              </button>
            </div>
          );
        })}
      </div>
    </section>
  );
}

function QueuePanel({
  jobs,
  selectedJobId,
  onSelectJob,
  onCancelJob,
}: {
  jobs: DesktopJob[];
  selectedJobId: string | null;
  onSelectJob: (jobId: string) => void;
  onCancelJob: (jobId: string) => void;
}) {
  return (
    <section className="queue-card">
      <div className="queue-header">
        <div>
          <h3>Queue</h3>
          <p>{jobs.length} waiting</p>
        </div>
        <span className="queue-count-pill">{jobs.length} queued</span>
      </div>
      <div className="queue-list">
        {jobs.map((job) => (
          <div
            key={job.job_id}
            className={`queue-row ${selectedJobId === job.job_id ? "selected" : ""}`}
          >
            <button
              type="button"
              className="queue-row-main"
              onClick={() => onSelectJob(job.job_id)}
            >
              <StatusIcon status={job.status} />
              <span>
                <strong>{job.file_name}</strong>
                <small>{queueJobSubtitle(job)}</small>
              </span>
            </button>
            <div className="queue-row-actions">
              <button
                className="icon-button"
                type="button"
                onClick={() => onCancelJob(job.job_id)}
                aria-label={`Cancel ${job.file_name}`}
              >
                <CircleStop size={17} />
              </button>
            </div>
          </div>
        ))}
      </div>
    </section>
  );
}
