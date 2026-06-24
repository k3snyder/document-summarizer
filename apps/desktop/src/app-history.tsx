import { FileJson, FileText, Trash2 } from "lucide-react";
import { StatusIcon } from "./app-common";
import { formatDuration, labelize } from "./app-core";
import { OutputViewer } from "./app-output";
import { DesktopJob } from "./types";

export function HistoryView({
  jobs,
  selectedJobId,
  onSelect,
  onDelete,
  onExport,
}: {
  jobs: DesktopJob[];
  selectedJobId: string | null;
  onSelect: (id: string) => void;
  onDelete: (id: string) => void;
  onExport: (job: DesktopJob, kind: "markdown" | "json") => void;
}) {
  const selectedJob = jobs.find((job) => job.job_id === selectedJobId) ?? null;
  return (
    <div className="history-grid">
      <section className="panel">
        <div className="panel-header">
          <div>
            <h3>Session History</h3>
            <p>History is stored in ~/.summarizer/history.json.</p>
          </div>
        </div>
        <div className="job-list">
          {jobs.map((job) => (
            <button
              key={job.job_id}
              className={`job-row ${selectedJobId === job.job_id ? "selected" : ""}`}
              onClick={() => onSelect(job.job_id)}
            >
              <StatusIcon status={job.status} />
              <span>
                <strong>{job.file_name}</strong>
                <small>
                  {labelize(job.status)} / {formatDuration(job.duration_ms)}
                </small>
              </span>
            </button>
          ))}
          {!jobs.length && <p className="muted">No jobs in this session.</p>}
        </div>
      </section>
      <section className="panel output-panel">
        {selectedJob?.output ? (
          <>
            <div
              className="history-output-toolbar"
              aria-label="Selected job actions"
            >
              <button
                className="button secondary compact"
                onClick={() => onExport(selectedJob, "json")}
              >
                <FileJson size={16} /> Save JSON
              </button>
              <button
                className="button danger compact"
                onClick={() => onDelete(selectedJob.job_id)}
              >
                <Trash2 size={16} /> Delete
              </button>
            </div>
            <OutputViewer
              job={selectedJob}
              onExport={onExport}
              onProcessAnother={() => undefined}
              showCompletionActions={false}
            />
          </>
        ) : selectedJob ? (
          <div className="empty-state">
            <StatusIcon status={selectedJob.status} />
            <h3>{selectedJob.file_name}</h3>
            <p>
              {selectedJob.error ?? "Output is not available for this job."}
            </p>
            {selectedJob.status !== "processing" && (
              <button
                className="button danger"
                onClick={() => onDelete(selectedJob.job_id)}
              >
                <Trash2 size={16} /> Delete
              </button>
            )}
          </div>
        ) : (
          <div className="empty-state">
            <FileText size={40} />
            <h3>Select a job</h3>
            <p>Completed outputs can be reviewed and exported here.</p>
          </div>
        )}
      </section>
    </div>
  );
}
