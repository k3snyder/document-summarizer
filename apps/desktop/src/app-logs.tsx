import { Channel, invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import {
  Download,
  Eraser,
  Filter,
  FolderOpen,
  ScrollText,
  Search,
  Trash2,
} from "lucide-react";
import * as React from "react";
import {
  errorMessage,
  formatBytes,
  formatDateTime,
  formatLogTimestamp,
  safeJson,
} from "./app-core";
import { LogEvent, LogFileMeta, LogLevel, LogPathInfo } from "./types";

export function LogsView({
  onError,
  onNotice,
}: {
  onError: (message: string | null) => void;
  onNotice: (message: string | null) => void;
}) {
  const [events, setEvents] = React.useState<LogEvent[]>([]);
  const [files, setFiles] = React.useState<LogFileMeta[]>([]);
  const [paths, setPaths] = React.useState<LogPathInfo | null>(null);
  const [selectedFile, setSelectedFile] = React.useState<string>("");
  const [mode, setMode] = React.useState<"live" | "file">("live");
  const [levelFilter, setLevelFilter] = React.useState<"all" | LogLevel>("all");
  const [sourceFilter, setSourceFilter] = React.useState("all");
  const [query, setQuery] = React.useState("");
  const [jobFilter, setJobFilter] = React.useState("");

  const refreshFiles = React.useCallback(async () => {
    const nextFiles = await invoke<LogFileMeta[]>("logs_list_files");
    setFiles(nextFiles);
    setSelectedFile((current) => {
      if (!current) return nextFiles[0]?.name || "";
      return nextFiles.some((file) => file.name === current)
        ? current
        : nextFiles[0]?.name || "";
    });
  }, []);

  React.useEffect(() => {
    let mounted = true;
    let subscriptionId: number | null = null;
    const channel = new Channel<LogEvent>();
    channel.onmessage = (event) => {
      if (!mounted) return;
      setEvents((current) => [...current, event].slice(-2_000));
    };

    void Promise.all([
      invoke<LogPathInfo>("logs_get_paths").then(setPaths),
      refreshFiles(),
      invoke<LogEvent[]>("logs_ring", { limit: 500 }).then((ring) =>
        setEvents(ring),
      ),
      invoke<number>("logs_subscribe", { channel, backfill: 0 }).then((id) => {
        if (mounted) {
          subscriptionId = id;
        } else {
          void invoke("logs_unsubscribe", { subscriptionId: id });
        }
      }),
    ]).catch((err) => onError(errorMessage(err)));

    const fileRefreshTimer = window.setInterval(() => {
      void refreshFiles().catch((err) => {
        if (mounted) onError(errorMessage(err));
      });
    }, 10_000);

    return () => {
      mounted = false;
      if (subscriptionId !== null) {
        void invoke("logs_unsubscribe", { subscriptionId });
      }
      window.clearInterval(fileRefreshTimer);
    };
  }, [onError, refreshFiles]);

  async function loadLiveRing() {
    try {
      const ring = await invoke<LogEvent[]>("logs_ring", { limit: 500 });
      setEvents(ring);
      setMode("live");
      await refreshFiles();
      onError(null);
    } catch (err) {
      onError(errorMessage(err));
    }
  }

  async function viewLogFile(fileName: string) {
    if (!fileName) return;
    try {
      const fileEvents = await invoke<LogEvent[]>("logs_read_file", {
        name: fileName,
        maxBytes: 1_048_576,
        fromEnd: true,
      });
      setSelectedFile(fileName);
      setEvents(fileEvents);
      setMode("file");
      onError(null);
    } catch (err) {
      onError(errorMessage(err));
    }
  }

  async function exportLogs() {
    const outputPath = await save({
      defaultPath: `summarizer-logs-${new Date().toISOString().slice(0, 10)}.zip`,
      filters: [{ name: "Zip", extensions: ["zip"] }],
    });
    if (!outputPath) return;
    try {
      const savedPath = await invoke<string>("logs_export", { outputPath });
      onNotice(`Saved logs to ${savedPath}.`);
      onError(null);
    } catch (err) {
      onError(errorMessage(err));
    }
  }

  async function clearRing() {
    try {
      await invoke("logs_clear_ring");
      if (mode === "live") setEvents([]);
      onNotice("Log ring cleared.");
      onError(null);
    } catch (err) {
      onError(errorMessage(err));
    }
  }

  async function deleteSelectedFile() {
    if (!selectedFile) return;
    try {
      const deletedFile = selectedFile;
      const nextFiles = await invoke<LogFileMeta[]>("logs_delete_file", {
        name: selectedFile,
      });
      const nextSelected = nextFiles[0]?.name ?? "";
      setFiles(nextFiles);
      setSelectedFile(nextSelected);
      const ring = await invoke<LogEvent[]>("logs_ring", { limit: 500 });
      setEvents(ring);
      setMode("live");
      onNotice(`Deleted ${deletedFile}.`);
      onError(null);
    } catch (err) {
      onError(errorMessage(err));
    }
  }

  const filteredEvents = React.useMemo(() => {
    const needle = query.trim().toLowerCase();
    const jobNeedle = jobFilter.trim().toLowerCase();
    return events.filter((event) => {
      if (levelFilter !== "all" && event.level !== levelFilter) return false;
      if (sourceFilter !== "all" && event.source !== sourceFilter) return false;
      if (jobNeedle && !(event.jobId ?? "").toLowerCase().includes(jobNeedle))
        return false;
      if (!needle) return true;
      return [
        event.message,
        event.target,
        event.stage ?? "",
        event.jobId ?? "",
        safeJson(event.fields),
      ].some((value) => value.toLowerCase().includes(needle));
    });
  }, [events, jobFilter, levelFilter, query, sourceFilter]);

  const { jobLogFiles, desktopLogFiles } = React.useMemo(() => {
    return files.reduce(
      (groups, file) => {
        if (file.name.startsWith("job-")) {
          groups.jobLogFiles.push(file);
        } else {
          groups.desktopLogFiles.push(file);
        }
        return groups;
      },
      {
        jobLogFiles: [] as LogFileMeta[],
        desktopLogFiles: [] as LogFileMeta[],
      },
    );
  }, [files]);

  const renderLogFileRow = (file: LogFileMeta) => (
    <button
      key={file.name}
      className={`log-file-row ${selectedFile === file.name ? "selected" : ""}`}
      onClick={() => void viewLogFile(file.name)}
      aria-label={`View file ${file.name}`}
      title="View file"
    >
      <FolderOpen size={16} />
      <span>
        <strong>{file.name}</strong>
        <small>
          {formatBytes(file.sizeBytes)} / {formatDateTime(file.modifiedAt)}
        </small>
      </span>
    </button>
  );

  return (
    <div className="logs-grid">
      <section className="panel logs-side-panel">
        <div className="panel-header">
          <div>
            <h3>Log Files</h3>
            <p>{paths?.logDir ?? "~/.summarizer/logs"}</p>
          </div>
          <div className="logs-actions log-file-actions">
            <button className="button secondary compact" onClick={loadLiveRing}>
              <ScrollText size={15} />
              Live Stream
            </button>
            <button
              className="button danger compact"
              onClick={deleteSelectedFile}
              disabled={!selectedFile}
            >
              <Trash2 size={15} />
              Delete
            </button>
          </div>
        </div>
        <div className="log-file-list">
          {!!jobLogFiles.length && (
            <div className="log-file-section">
              <div className="log-file-section-header">
                <h4>Job Logs</h4>
                <span>{jobLogFiles.length}</span>
              </div>
              <div className="log-file-section-list">
                {jobLogFiles.map(renderLogFileRow)}
              </div>
            </div>
          )}
          {!!desktopLogFiles.length && (
            <div className="log-file-section">
              <div className="log-file-section-header">
                <h4>Desktop Logs</h4>
                <span>{desktopLogFiles.length}</span>
              </div>
              <div className="log-file-section-list">
                {desktopLogFiles.map(renderLogFileRow)}
              </div>
            </div>
          )}
          {!files.length && <p className="muted">No log files yet.</p>}
        </div>
      </section>

      <section className="panel logs-main-panel">
        <div className="panel-header logs-header">
          <div>
            <h3>
              {mode === "live" ? "Live Events" : selectedFile || "File Events"}
            </h3>
            <p>
              {filteredEvents.length} of {events.length} events
            </p>
          </div>
          <div className="logs-actions">
            <button className="button secondary compact" onClick={exportLogs}>
              <Download size={15} />
              Export
            </button>
            {mode === "live" && (
              <button className="button danger compact" onClick={clearRing}>
                <Eraser size={15} />
                Clear
              </button>
            )}
          </div>
        </div>

        <div className="log-filter-bar">
          <label className="input-field log-search-field">
            <span>
              <Search size={13} /> Search
            </span>
            <input
              value={query}
              onChange={(event) => setQuery(event.currentTarget.value)}
            />
          </label>
          <label className="input-field">
            <span>
              <Filter size={13} /> Level
            </span>
            <select
              value={levelFilter}
              onChange={(event) =>
                setLevelFilter(event.currentTarget.value as "all" | LogLevel)
              }
            >
              <option value="all">all</option>
              <option value="trace">trace</option>
              <option value="debug">debug</option>
              <option value="info">info</option>
              <option value="warn">warn</option>
              <option value="error">error</option>
            </select>
          </label>
          <label className="input-field">
            <span>Source</span>
            <select
              value={sourceFilter}
              onChange={(event) => setSourceFilter(event.currentTarget.value)}
            >
              <option value="all">all</option>
              <option value="desktop">desktop</option>
              <option value="frontend">frontend</option>
              <option value="dev_service">dev service</option>
            </select>
          </label>
          <label className="input-field">
            <span>Job</span>
            <input
              value={jobFilter}
              onChange={(event) => setJobFilter(event.currentTarget.value)}
            />
          </label>
        </div>

        <div className="log-event-list">
          {filteredEvents.map((event) => (
            <LogEventRow
              key={`${event.seq}-${event.timestamp}-${event.message}`}
              event={event}
            />
          ))}
          {!filteredEvents.length && (
            <div className="empty-state">
              <ScrollText size={40} />
              <h3>No events</h3>
            </div>
          )}
        </div>
      </section>
    </div>
  );
}

function LogEventRow({ event }: { event: LogEvent }) {
  return (
    <details className={`log-event-row ${event.level}`}>
      <summary>
        <span className={`log-level ${event.level}`}>{event.level}</span>
        <span className="log-time">{formatLogTimestamp(event.timestamp)}</span>
        <span className="log-message">{event.message || event.target}</span>
        <small>
          {event.source}
          {event.stage ? ` / ${event.stage}` : ""}
        </small>
      </summary>
      <div className="log-event-details">
        <code>
          {event.target}
          {event.line ? `:${event.line}` : ""}
        </code>
        {event.jobId && <code>job {event.jobId}</code>}
        <pre>{safeJson(event.fields)}</pre>
      </div>
    </details>
  );
}
