import { invoke } from "@tauri-apps/api/core";
import { Save } from "lucide-react";
import * as React from "react";
import {
  NumberField,
  ProviderBlock,
  ProviderVisibilityGroup,
  SelectField,
  SettingsToggle,
  SwitchRow,
  TextField,
} from "./app-common";
import {
  DEFAULT_LOGGING_SETTINGS,
  DEFAULT_PROVIDER_VISIBILITY,
  DEFAULT_UPDATE_SETTINGS,
  displaySettingsPath,
  enabledCount,
  errorMessage,
  REASONING_EFFORT_OPTIONS,
  SUMMARIZER_PROVIDER_OPTIONS,
  VISION_PROVIDER_OPTIONS,
} from "./app-core";
import {
  DEFAULT_PIPELINE_CONFIG,
  DesktopSettings,
  LogLevel,
  PipelineConfig,
  ProviderSettings,
  ReasoningEffort,
  SummarizerProvider,
  ThemePreference,
  VisibleVisionProvider,
} from "./types";

export function SettingsView({
  settings,
  settingsPath,
  appVersion,
  onSettingsChange,
  onError,
  onNotice,
}: {
  settings: DesktopSettings;
  settingsPath: string;
  appVersion: string;
  onSettingsChange: (settings: DesktopSettings) => void;
  onError: (message: string | null) => void;
  onNotice: (message: string | null) => void;
}) {
  const [draft, setDraft] = React.useState<DesktopSettings>(settings);

  React.useEffect(() => setDraft(settings), [settings]);

  function updateTheme(theme: ThemePreference) {
    setDraft((current) => ({
      ...current,
      appearance: { ...current.appearance, theme },
    }));
  }

  function updateLogging<K extends keyof DesktopSettings["logging"]>(
    field: K,
    value: DesktopSettings["logging"][K],
  ) {
    setDraft((current) => ({
      ...current,
      logging: {
        ...(current.logging ?? DEFAULT_LOGGING_SETTINGS),
        [field]: value,
      },
    }));
  }

  function updateProvider<P extends keyof ProviderSettings>(
    provider: P,
    field: keyof ProviderSettings[P],
    value: string | number,
  ) {
    setDraft((current) => ({
      ...current,
      providers: {
        ...current.providers,
        [provider]: {
          ...current.providers[provider],
          [field]: value,
        },
      },
    }));
  }

  function updatePipelineDefault<K extends keyof PipelineConfig>(
    field: K,
    value: PipelineConfig[K],
  ) {
    setDraft((current) => ({
      ...current,
      pipeline_defaults: {
        ...DEFAULT_PIPELINE_CONFIG,
        ...(current.pipeline_defaults ?? {}),
        [field]: value,
      },
    }));
  }

  function updateUpdates<K extends keyof DesktopSettings["updates"]>(
    field: K,
    value: DesktopSettings["updates"][K],
  ) {
    setDraft((current) => ({
      ...current,
      updates: {
        ...DEFAULT_UPDATE_SETTINGS,
        ...(current.updates ?? {}),
        [field]: value,
      },
    }));
  }

  function updateVisionProviderVisibility(
    group: "vision" | "classifier",
    provider: VisibleVisionProvider,
    enabled: boolean,
  ) {
    setDraft((current) => {
      const visibility =
        current.provider_visibility ?? DEFAULT_PROVIDER_VISIBILITY;
      const groupSettings = visibility[group];
      if (
        !enabled &&
        enabledCount(groupSettings) <= 1 &&
        groupSettings[provider]
      )
        return current;
      return {
        ...current,
        provider_visibility: {
          ...visibility,
          [group]: {
            ...groupSettings,
            [provider]: enabled,
          },
        },
      };
    });
  }

  function updateSummarizerProviderVisibility(
    provider: SummarizerProvider,
    enabled: boolean,
  ) {
    setDraft((current) => {
      const visibility =
        current.provider_visibility ?? DEFAULT_PROVIDER_VISIBILITY;
      const groupSettings = visibility.summarizer;
      if (
        !enabled &&
        enabledCount(groupSettings) <= 1 &&
        groupSettings[provider]
      )
        return current;
      return {
        ...current,
        provider_visibility: {
          ...visibility,
          summarizer: {
            ...groupSettings,
            [provider]: enabled,
          },
        },
      };
    });
  }

  async function saveSettings() {
    try {
      const saved = await invoke<DesktopSettings>("save_settings", {
        settings: draft,
      });
      onSettingsChange(saved);
      onNotice(`Settings saved to ${displaySettingsPath(settingsPath)}.`);
      onError(null);
    } catch (err) {
      onError(errorMessage(err));
    }
  }

  return (
    <section className="panel settings-panel">
      <div className="panel-header">
        <div>
          <h3>Providers</h3>
          <p>Settings file: {displaySettingsPath(settingsPath)}</p>
        </div>
        <button className="button primary" onClick={saveSettings}>
          <Save size={16} />
          Save
        </button>
      </div>

      <div className="settings-section">
        <h4>Appearance</h4>
        <SelectField
          label="Theme"
          value={draft.appearance.theme}
          options={["light", "system", "dark"]}
          onChange={(value) => updateTheme(value as ThemePreference)}
        />
      </div>

      <div className="settings-section">
        <div className="section-heading">
          <div>
            <h4>Updates</h4>
          </div>
        </div>
        <SwitchRow
          label="Check for updates on launch"
          checked={(draft.updates ?? DEFAULT_UPDATE_SETTINGS).enabled}
          onChange={(checked) => updateUpdates("enabled", checked)}
        />
        <div className="settings-version-row">
          <strong>Current version</strong>
          <span className="version-pill" title="Installed app version">
            v{appVersion || "—"}
          </span>
        </div>
      </div>

      <div className="settings-section logging-settings-section">
        <div className="section-heading">
          <div>
            <h4>Logging</h4>
            <span>{draft.logging.enabled ? "Enabled" : "Disabled"}</span>
          </div>
        </div>
        <div className="field-grid">
          <SelectField
            label="Level"
            value={draft.logging.level}
            options={["trace", "debug", "info", "warn", "error"]}
            onChange={(value) => updateLogging("level", value as LogLevel)}
          />
          <NumberField
            label="Retention days"
            value={draft.logging.retention_days}
            min={1}
            step={1}
            onChange={(value) => updateLogging("retention_days", value)}
          />
          <NumberField
            label="Max file MB"
            value={draft.logging.max_file_mb}
            min={1}
            step={5}
            onChange={(value) => updateLogging("max_file_mb", value)}
          />
        </div>
        <div className="provider-toggle-grid logging-toggle-grid">
          <SettingsToggle
            label="Logging"
            checked={draft.logging.enabled}
            onChange={(checked) => updateLogging("enabled", checked)}
          />
          <SettingsToggle
            label="Frontend"
            checked={draft.logging.capture_frontend}
            onChange={(checked) => updateLogging("capture_frontend", checked)}
          />
          <SettingsToggle
            label="Dev Services"
            checked={draft.logging.capture_dev_services}
            onChange={(checked) =>
              updateLogging("capture_dev_services", checked)
            }
          />
          <SettingsToggle
            label="Redaction"
            checked={draft.logging.redact_secrets}
            onChange={(checked) => updateLogging("redact_secrets", checked)}
          />
        </div>
      </div>

      <div className="settings-section">
        <div className="section-heading">
          <div>
            <h4>Summarization Budget</h4>
            <span>Defaults for new jobs.</span>
          </div>
        </div>
        <div className="field-grid">
          <NumberField
            label="Max tokens/page"
            value={draft.pipeline_defaults.max_tokens_per_page}
            min={1000}
            step={1000}
            onChange={(value) =>
              updatePipelineDefault("max_tokens_per_page", value)
            }
          />
          <NumberField
            label="Max seconds/page"
            value={draft.pipeline_defaults.max_seconds_per_page}
            min={5}
            step={5}
            onChange={(value) =>
              updatePipelineDefault("max_seconds_per_page", value)
            }
          />
        </div>
      </div>

      <div className="settings-section provider-visibility-section">
        <div className="section-heading">
          <div>
            <h4>Provider Visibility</h4>
            <span>Choose which providers appear in the Process wizard.</span>
          </div>
        </div>
        <ProviderVisibilityGroup
          title="Vision Providers"
          description="Shown when visual page analysis is enabled."
          options={VISION_PROVIDER_OPTIONS}
          visibility={
            (draft.provider_visibility ?? DEFAULT_PROVIDER_VISIBILITY).vision
          }
          onChange={(provider, enabled) =>
            updateVisionProviderVisibility("vision", provider, enabled)
          }
        />
        <ProviderVisibilityGroup
          title="Classifier Providers"
          description="Shown in advanced vision mode for visual page classification."
          options={VISION_PROVIDER_OPTIONS}
          visibility={
            (draft.provider_visibility ?? DEFAULT_PROVIDER_VISIBILITY)
              .classifier
          }
          onChange={(provider, enabled) =>
            updateVisionProviderVisibility("classifier", provider, enabled)
          }
        />
        <ProviderVisibilityGroup
          title="Summarizer Providers"
          description="Shown when choosing the LLM provider for summaries."
          options={SUMMARIZER_PROVIDER_OPTIONS}
          visibility={
            (draft.provider_visibility ?? DEFAULT_PROVIDER_VISIBILITY)
              .summarizer
          }
          onChange={updateSummarizerProviderVisibility}
        />
      </div>

      <ProviderBlock title="llama.cpp">
        <TextField
          label="Text base URL"
          value={draft.providers.llama_cpp.base_url}
          onChange={(value) => updateProvider("llama_cpp", "base_url", value)}
        />
        <TextField
          label="Vision base URL"
          value={draft.providers.llama_cpp.vision_base_url}
          onChange={(value) =>
            updateProvider("llama_cpp", "vision_base_url", value)
          }
        />
        <TextField
          label="API key"
          type="password"
          value={draft.providers.llama_cpp.api_key}
          onChange={(value) => updateProvider("llama_cpp", "api_key", value)}
        />
        <TextField
          label="Model"
          value={draft.providers.llama_cpp.model}
          onChange={(value) => updateProvider("llama_cpp", "model", value)}
        />
        <TextField
          label="Model tier 2"
          value={draft.providers.llama_cpp.model_2}
          onChange={(value) => updateProvider("llama_cpp", "model_2", value)}
        />
        <TextField
          label="Model tier 3"
          value={draft.providers.llama_cpp.model_3}
          onChange={(value) => updateProvider("llama_cpp", "model_3", value)}
        />
        <TextField
          label="Vision model"
          value={draft.providers.llama_cpp.vision_model}
          onChange={(value) =>
            updateProvider("llama_cpp", "vision_model", value)
          }
        />
      </ProviderBlock>

      <ProviderBlock title="OpenAI">
        <TextField
          label="Base URL"
          value={draft.providers.openai.base_url}
          onChange={(value) => updateProvider("openai", "base_url", value)}
        />
        <TextField
          label="API key"
          type="password"
          value={draft.providers.openai.api_key}
          onChange={(value) => updateProvider("openai", "api_key", value)}
        />
        <TextField
          label="Model"
          value={draft.providers.openai.model}
          onChange={(value) => updateProvider("openai", "model", value)}
        />
        <TextField
          label="Model tier 2"
          value={draft.providers.openai.model_2}
          onChange={(value) => updateProvider("openai", "model_2", value)}
        />
        <TextField
          label="Model tier 3"
          value={draft.providers.openai.model_3}
          onChange={(value) => updateProvider("openai", "model_3", value)}
        />
        <TextField
          label="Vision model"
          value={draft.providers.openai.vision_model}
          onChange={(value) => updateProvider("openai", "vision_model", value)}
        />
      </ProviderBlock>

      <ProviderBlock title="Ollama">
        <TextField
          label="OpenAI-compatible base URL"
          value={draft.providers.ollama.openai_base_url}
          onChange={(value) =>
            updateProvider("ollama", "openai_base_url", value)
          }
        />
        <TextField
          label="API key"
          type="password"
          value={draft.providers.ollama.api_key}
          onChange={(value) => updateProvider("ollama", "api_key", value)}
        />
        <TextField
          label="Model"
          value={draft.providers.ollama.model}
          onChange={(value) => updateProvider("ollama", "model", value)}
        />
        <TextField
          label="Model tier 2"
          value={draft.providers.ollama.model_2}
          onChange={(value) => updateProvider("ollama", "model_2", value)}
        />
        <TextField
          label="Model tier 3"
          value={draft.providers.ollama.model_3}
          onChange={(value) => updateProvider("ollama", "model_3", value)}
        />
        <TextField
          label="Vision model"
          value={draft.providers.ollama.vision_model}
          onChange={(value) => updateProvider("ollama", "vision_model", value)}
        />
      </ProviderBlock>

      <ProviderBlock title="CLI Providers">
        <TextField
          label="Codex executable"
          value={draft.providers.codex.executable}
          onChange={(value) => updateProvider("codex", "executable", value)}
        />
        <TextField
          label="Codex args"
          value={draft.providers.codex.args}
          onChange={(value) => updateProvider("codex", "args", value)}
        />
        <SelectField
          label="Codex reasoning effort"
          value={draft.providers.codex.reasoning_effort}
          options={REASONING_EFFORT_OPTIONS}
          onChange={(value) =>
            updateProvider(
              "codex",
              "reasoning_effort",
              value as ReasoningEffort,
            )
          }
        />
        <NumberField
          label="Codex timeout seconds"
          value={draft.providers.codex.timeout_seconds}
          min={1}
          step={30}
          onChange={(value) =>
            updateProvider("codex", "timeout_seconds", value)
          }
        />
        <TextField
          label="Claude executable"
          value={draft.providers.claude.executable}
          onChange={(value) => updateProvider("claude", "executable", value)}
        />
        <TextField
          label="Claude args"
          value={draft.providers.claude.args}
          onChange={(value) => updateProvider("claude", "args", value)}
        />
        <SelectField
          label="Claude reasoning effort"
          value={draft.providers.claude.reasoning_effort}
          options={REASONING_EFFORT_OPTIONS}
          onChange={(value) =>
            updateProvider(
              "claude",
              "reasoning_effort",
              value as ReasoningEffort,
            )
          }
        />
        <NumberField
          label="Claude timeout seconds"
          value={draft.providers.claude.timeout_seconds}
          min={1}
          step={30}
          onChange={(value) =>
            updateProvider("claude", "timeout_seconds", value)
          }
        />
        <TextField
          label="Grok executable"
          value={draft.providers.grok.executable}
          onChange={(value) => updateProvider("grok", "executable", value)}
        />
        <TextField
          label="Grok args"
          value={draft.providers.grok.args}
          onChange={(value) => updateProvider("grok", "args", value)}
        />
        <NumberField
          label="Grok timeout seconds"
          value={draft.providers.grok.timeout_seconds}
          min={1}
          step={30}
          onChange={(value) => updateProvider("grok", "timeout_seconds", value)}
        />
        <TextField
          label="Copilot executable"
          value={draft.providers.copilot.executable}
          onChange={(value) => updateProvider("copilot", "executable", value)}
        />
        <TextField
          label="Copilot args"
          value={draft.providers.copilot.args}
          onChange={(value) => updateProvider("copilot", "args", value)}
        />
        <NumberField
          label="Copilot timeout seconds"
          value={draft.providers.copilot.timeout_seconds}
          min={1}
          step={30}
          onChange={(value) =>
            updateProvider("copilot", "timeout_seconds", value)
          }
        />
      </ProviderBlock>
    </section>
  );
}
