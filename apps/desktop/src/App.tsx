import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";
import {
  FilePlus2,
  FileText,
  History,
  ScrollText,
  Settings,
} from "lucide-react";
import * as React from "react";
import type { ProviderAvailabilityMap, View } from "./app-ui";
import {
  Banner,
  DEFAULT_PROVIDER_VISIBILITY,
  Header,
  HistoryView,
  LogsView,
  NavButton,
  ProcessView,
  SettingsView,
  applyTheme,
  basename,
  errorMessage,
  isSupportedInputPath,
  normalizeDesktopSettings,
  normalizedConfigForProviderVisibility,
  providerAvailabilityKey,
  stripExtension,
  upsertJob,
} from "./app-ui";
import type {
  DesktopJob,
  DesktopJobProgress,
  DesktopSettings,
  PipelineConfig,
  ProviderAvailability,
} from "./types";
import { DEFAULT_PIPELINE_CONFIG } from "./types";

export default function App() {
  const [view, setView] = React.useState<View>("process");
  const [selectedFiles, setSelectedFiles] = React.useState<string[]>([]);
  const [config, setConfig] = React.useState<PipelineConfig>({
    ...DEFAULT_PIPELINE_CONFIG,
  });
  const [settings, setSettings] = React.useState<DesktopSettings | null>(null);
  const [settingsPath, setSettingsPath] = React.useState<string>("");
  const [providerAvailability, setProviderAvailability] =
    React.useState<ProviderAvailabilityMap>({});
  const [jobs, setJobs] = React.useState<DesktopJob[]>([]);
  const [jobProgress, setJobProgress] = React.useState<
    Record<string, DesktopJobProgress>
  >({});
  const [selectedProcessJobId, setSelectedProcessJobId] = React.useState<
    string | null
  >(null);
  const [selectedHistoryJobId, setSelectedHistoryJobId] = React.useState<
    string | null
  >(null);
  const [notice, setNotice] = React.useState<string | null>(null);
  const [error, setError] = React.useState<string | null>(null);
  const [isFileDragOver, setIsFileDragOver] = React.useState(false);
  const providerAvailabilityRequestRef = React.useRef(0);

  const refreshHistory = React.useCallback(async () => {
    const nextJobs = await invoke<DesktopJob[]>("history");
    setJobs(nextJobs);
    setSelectedHistoryJobId((current) => {
      if (current && nextJobs.some((job) => job.job_id === current))
        return current;
      return nextJobs[0]?.job_id ?? null;
    });
    setSelectedProcessJobId((current) => {
      if (current && nextJobs.some((job) => job.job_id === current))
        return current;
      return (
        nextJobs.find((job) => job.status === "processing")?.job_id ??
        nextJobs.find((job) => job.status === "queued")?.job_id ??
        null
      );
    });
  }, []);

  const refreshProviderAvailability = React.useCallback(
    async (currentSettings: DesktopSettings) => {
      const requestId = ++providerAvailabilityRequestRef.current;
      const availability = await invoke<ProviderAvailability[]>(
        "provider_availability",
        {
          settings: currentSettings,
        },
      );
      if (requestId !== providerAvailabilityRequestRef.current) return;
      setProviderAvailability(
        Object.fromEntries(
          availability.map((item) => [
            providerAvailabilityKey(item.role, item.provider),
            item,
          ]),
        ),
      );
    },
    [],
  );

  React.useEffect(() => {
    void Promise.all([
      invoke<DesktopSettings>("load_settings").then((loaded) => {
        const normalized = normalizeDesktopSettings(loaded);
        setSettings(normalized);
        setConfig(
          normalizedConfigForProviderVisibility(
            normalized.pipeline_defaults,
            normalized.provider_visibility,
          ) ?? normalized.pipeline_defaults,
        );
      }),
      invoke<string>("settings_file_path").then(setSettingsPath),
      refreshHistory(),
    ]).catch((err) => setError(errorMessage(err)));
  }, [refreshHistory]);

  React.useEffect(() => {
    if (!settings) return;
    applyTheme(settings.appearance.theme);
  }, [settings]);

  React.useEffect(() => {
    if (!settings) return;
    setConfig(
      (current) =>
        normalizedConfigForProviderVisibility(
          current,
          settings.provider_visibility,
        ) ?? current,
    );
  }, [settings]);

  React.useEffect(() => {
    if (!settings) return;
    let canceled = false;
    const refresh = () => {
      void refreshProviderAvailability(settings).catch((err) => {
        if (!canceled) setError(errorMessage(err));
      });
    };
    refresh();
    const interval = window.setInterval(refresh, 30_000);
    return () => {
      canceled = true;
      providerAvailabilityRequestRef.current += 1;
      window.clearInterval(interval);
    };
  }, [refreshProviderAvailability, settings]);

  React.useEffect(() => {
    if (!settings) return;
    let canceled = false;
    const refresh = () => {
      void refreshProviderAvailability(settings).catch((err) => {
        if (!canceled) setError(errorMessage(err));
      });
    };
    const handleVisibilityChange = () => {
      if (document.visibilityState === "visible") refresh();
    };
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", handleVisibilityChange);
    return () => {
      canceled = true;
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", handleVisibilityChange);
    };
  }, [refreshProviderAvailability, settings]);

  React.useEffect(() => {
    if (!settings || view !== "process") return;
    void refreshProviderAvailability(settings).catch((err) =>
      setError(errorMessage(err)),
    );
  }, [refreshProviderAvailability, settings, view]);

  React.useEffect(() => {
    let mounted = true;
    const unlisten: Array<() => void> = [];
    void Promise.all([
      listen<DesktopJob>("job:queued", (event) => {
        upsertJob(setJobs, event.payload);
        setSelectedProcessJobId((current) => current ?? event.payload.job_id);
      }),
      listen<DesktopJob>("job:started", (event) => {
        upsertJob(setJobs, event.payload);
        setSelectedProcessJobId(event.payload.job_id);
        setJobProgress((current) => {
          const next = { ...current };
          delete next[event.payload.job_id];
          return next;
        });
      }),
      listen<DesktopJobProgress>("job:progress", (event) => {
        setJobProgress((current) => ({
          ...current,
          [event.payload.job_id]: event.payload,
        }));
      }),
      listen<DesktopJob>("job:updated", (event) => {
        upsertJob(setJobs, event.payload);
      }),
      listen<DesktopJob>("job:completed", (event) => {
        upsertJob(setJobs, event.payload);
        setSelectedProcessJobId((current) => current ?? event.payload.job_id);
        setNotice(`${event.payload.file_name} completed.`);
      }),
      listen<DesktopJob>("job:failed", (event) => {
        upsertJob(setJobs, event.payload);
        setSelectedProcessJobId((current) => current ?? event.payload.job_id);
        setError(event.payload.error ?? "Processing failed.");
      }),
      listen<DesktopJob>("job:canceled", (event) => {
        upsertJob(setJobs, event.payload);
        setSelectedProcessJobId((current) => current ?? event.payload.job_id);
        setNotice(`${event.payload.file_name} canceled.`);
      }),
    ]).then((listeners) => {
      if (mounted) {
        unlisten.push(...listeners);
      } else {
        listeners.forEach((listener) => listener());
      }
    });
    return () => {
      mounted = false;
      unlisten.forEach((listener) => listener());
    };
  }, []);

  const addSelectedFiles = React.useCallback((paths: string[]) => {
    const supported = paths.filter(isSupportedInputPath);
    const unsupported = paths.filter((path) => !isSupportedInputPath(path));
    if (unsupported.length) {
      setError(
        `Unsupported files were not added: ${unsupported.map(basename).join(", ")}.`,
      );
    } else {
      setError(null);
    }
    if (!supported.length) return;
    setNotice(null);
    setSelectedProcessJobId(null);
    setSelectedFiles((current) => {
      const next = [...current];
      for (const path of supported) {
        if (!next.includes(path)) next.push(path);
      }
      return next;
    });
    setView("process");
  }, []);

  React.useEffect(() => {
    let mounted = true;
    let unlisten: (() => void) | null = null;

    void getCurrentWebview()
      .onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === "enter" || payload.type === "over") {
          setIsFileDragOver(true);
          return;
        }
        if (payload.type === "leave") {
          setIsFileDragOver(false);
          return;
        }
        setIsFileDragOver(false);
        addSelectedFiles(payload.paths);
      })
      .then((listener) => {
        if (mounted) {
          unlisten = listener;
        } else {
          listener();
        }
      })
      .catch(() => undefined);

    return () => {
      mounted = false;
      unlisten?.();
    };
  }, [addSelectedFiles]);

  const activeJob = jobs.find((job) => job.status === "processing") ?? null;
  const selectedProcessJob =
    jobs.find((job) => job.job_id === selectedProcessJobId) ?? null;

  async function chooseFiles() {
    setError(null);
    const selected = await open({
      multiple: true,
      filters: [
        {
          name: "Documents",
          extensions: ["pdf", "pptx", "docx", "txt", "md", "markdown"],
        },
      ],
    });
    if (typeof selected === "string") {
      addSelectedFiles([selected]);
    } else if (Array.isArray(selected)) {
      addSelectedFiles(selected);
    }
  }

  async function enqueueSelectedFiles() {
    if (!selectedFiles.length) {
      setError("Choose one or more PDF, PPTX, DOCX, TXT, or MD files first.");
      return;
    }
    setError(null);
    setNotice(null);
    try {
      if (settings) await refreshProviderAvailability(settings);
      const enqueuedJobs = await invoke<DesktopJob[]>("enqueue_jobs", {
        filePaths: selectedFiles,
        config,
      });
      for (const job of enqueuedJobs) upsertJob(setJobs, job);
      setSelectedProcessJobId(enqueuedJobs[0]?.job_id ?? null);
      setSelectedFiles([]);
      setNotice(
        `Added ${enqueuedJobs.length} document${enqueuedJobs.length === 1 ? "" : "s"} to the queue.`,
      );
    } catch (err) {
      setError(errorMessage(err));
    }
  }

  async function cancelActiveJob() {
    if (!activeJob) return;
    await cancelJob(activeJob.job_id);
  }

  async function cancelJob(jobId: string) {
    setError(null);
    try {
      const job = await invoke<DesktopJob>("cancel_job", { jobId });
      upsertJob(setJobs, job);
    } catch (err) {
      setError(errorMessage(err));
    }
  }

  async function deleteJob(jobId: string) {
    setError(null);
    try {
      const nextJobs = await invoke<DesktopJob[]>("delete_job", { jobId });
      setJobs(nextJobs);
      if (selectedProcessJobId === jobId) {
        setSelectedProcessJobId(
          nextJobs.find((job) => job.status === "processing")?.job_id ??
            nextJobs.find((job) => job.status === "queued")?.job_id ??
            null,
        );
      }
      if (selectedHistoryJobId === jobId)
        setSelectedHistoryJobId(nextJobs[0]?.job_id ?? null);
    } catch (err) {
      setError(errorMessage(err));
    }
  }

  async function exportJob(job: DesktopJob, kind: "markdown" | "json") {
    setError(null);
    const base = stripExtension(job.file_name);
    const path = await save({
      defaultPath:
        kind === "markdown" ? `${base}_summary.md` : `${base}_output.json`,
      filters:
        kind === "markdown"
          ? [{ name: "Markdown", extensions: ["md"] }]
          : [{ name: "JSON", extensions: ["json"] }],
    });
    if (!path) return;
    try {
      await invoke(
        kind === "markdown" ? "save_job_markdown" : "save_job_json",
        {
          jobId: job.job_id,
          outputPath: path,
        },
      );
      setNotice(
        `Saved ${kind === "markdown" ? "Markdown" : "JSON"} to ${path}.`,
      );
    } catch (err) {
      setError(errorMessage(err));
    }
  }

  function processAnother() {
    setSelectedFiles([]);
    setSelectedProcessJobId(null);
    setView("process");
  }

  const sidebarStatus = activeJob
    ? {
        state: "processing",
        label: "processing",
        title: `Processing ${activeJob.file_name}`,
      }
    : jobs.some((job) => job.status === "queued")
      ? {
          state: "queued",
          label: "Queued",
          title: "Documents are waiting to process.",
        }
      : {
          state: "ready",
          label: "Ready",
          title: "Ready to process a document.",
        };

  return (
    <main className="app-shell">
      <aside className="sidebar">
        <div className="brand-block">
          <div className="brand-icon">
            <FileText size={25} aria-hidden="true" />
          </div>
          <div>
            <h1>Document Summarizer</h1>
          </div>
        </div>

        <div className="nav-card">
          <nav className="nav-list" aria-label="Primary">
            <NavButton
              icon={<FilePlus2 size={19} />}
              active={view === "process"}
              onClick={() => setView("process")}
            >
              Process
            </NavButton>
            <NavButton
              icon={<History size={19} />}
              active={view === "history"}
              onClick={() => setView("history")}
            >
              History
            </NavButton>
            <NavButton
              icon={<ScrollText size={19} />}
              active={view === "logs"}
              onClick={() => setView("logs")}
            >
              Logs
            </NavButton>
          </nav>
        </div>

        <div className="sidebar-footer">
          <div
            className={`sidebar-status ${sidebarStatus.state}`}
            title={sidebarStatus.title}
          >
            <span className={`status-dot ${sidebarStatus.state}`} />
            <span>{sidebarStatus.label}</span>
          </div>
          <NavButton
            icon={<Settings size={19} />}
            active={view === "settings"}
            onClick={() => setView("settings")}
          >
            Settings
          </NavButton>
        </div>
      </aside>

      <section className="workspace">
        <Header view={view} activeJob={activeJob} onCancel={cancelActiveJob} />
        {notice && (
          <Banner
            tone="success"
            message={notice}
            onClose={() => setNotice(null)}
          />
        )}
        {error && (
          <Banner tone="error" message={error} onClose={() => setError(null)} />
        )}

        {view === "process" && (
          <ProcessView
            selectedFiles={selectedFiles}
            selectedJob={selectedProcessJob}
            selectedJobId={selectedProcessJobId}
            jobs={jobs}
            activeJob={activeJob}
            activeProgress={
              activeJob ? (jobProgress[activeJob.job_id] ?? null) : null
            }
            config={config}
            providerVisibility={
              settings?.provider_visibility ?? DEFAULT_PROVIDER_VISIBILITY
            }
            providerAvailability={providerAvailability}
            isFileDragOver={isFileDragOver}
            onChooseFiles={chooseFiles}
            onRemoveFile={(filePath) => {
              setSelectedFiles((current) =>
                current.filter((path) => path !== filePath),
              );
              setError(null);
            }}
            onClearFiles={() => {
              setSelectedFiles([]);
              setError(null);
            }}
            onEnqueue={enqueueSelectedFiles}
            onConfigChange={setConfig}
            onSelectJob={setSelectedProcessJobId}
            onCancelJob={cancelJob}
            onExport={exportJob}
            onProcessAnother={processAnother}
          />
        )}
        {view === "history" && (
          <HistoryView
            jobs={jobs}
            selectedJobId={selectedHistoryJobId}
            onSelect={setSelectedHistoryJobId}
            onDelete={deleteJob}
            onExport={exportJob}
          />
        )}
        {view === "logs" && (
          <LogsView onError={setError} onNotice={setNotice} />
        )}
        {view === "settings" && settings && (
          <SettingsView
            settings={settings}
            settingsPath={settingsPath}
            onSettingsChange={(nextSettings) => {
              const normalized = normalizeDesktopSettings(nextSettings);
              setSettings(normalized);
              setConfig(
                normalizedConfigForProviderVisibility(
                  normalized.pipeline_defaults,
                  normalized.provider_visibility,
                ) ?? normalized.pipeline_defaults,
              );
              applyTheme(nextSettings.appearance.theme);
            }}
            onError={setError}
            onNotice={setNotice}
          />
        )}
      </section>
    </main>
  );
}
