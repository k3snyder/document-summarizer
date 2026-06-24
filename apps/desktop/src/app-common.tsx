import {
  AlertTriangle,
  CheckCircle2,
  CircleStop,
  Clock,
  FilePlus2,
  History,
  ScrollText,
  Settings,
} from "lucide-react";
import * as React from "react";
import {
  enabledCount,
  labelize,
  providerAvailabilityKey,
  ProviderAvailabilityMap,
  View,
} from "./app-core";
import {
  DesktopJob,
  ProviderAvailability,
  ProviderAvailabilityRole,
} from "./types";

export function Header({
  view,
  activeJob,
  onCancel,
}: {
  view: View;
  activeJob: DesktopJob | null;
  onCancel: () => void;
}) {
  const title =
    view === "process"
      ? "Process"
      : view === "history"
        ? "History"
        : view === "logs"
          ? "Logs"
          : "Settings";
  const subtitle =
    view === "process"
      ? "Extract and structure content for AI-ready consumption."
      : view === "history"
        ? "Session output and exports"
        : view === "logs"
          ? "Runtime events and processing diagnostics"
          : "Providers and appearance";
  const HeaderIcon =
    view === "process"
      ? FilePlus2
      : view === "history"
        ? History
        : view === "logs"
          ? ScrollText
          : Settings;

  return (
    <header className="topbar">
      <div className="view-heading">
        <span className="header-icon">
          <HeaderIcon size={25} aria-hidden="true" />
        </span>
        <div>
          <h2>{title}</h2>
          <p>{subtitle}</p>
        </div>
      </div>
      {activeJob && (
        <div className="topbar-actions">
          <button className="button secondary" onClick={onCancel}>
            <CircleStop size={16} />
            Cancel
          </button>
        </div>
      )}
    </header>
  );
}

export function NavButton({
  active,
  icon,
  children,
  onClick,
}: {
  active: boolean;
  icon: React.ReactNode;
  children: React.ReactNode;
  onClick: () => void;
}) {
  return (
    <button
      className={`nav-button ${active ? "active" : ""}`}
      aria-current={active ? "page" : undefined}
      onClick={onClick}
    >
      {icon}
      {children}
    </button>
  );
}

export function SelectField({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: string;
  options: string[];
  onChange: (value: string) => void;
}) {
  return (
    <label className="input-field">
      <span>{label}</span>
      <select
        value={value}
        onChange={(event) => onChange(event.currentTarget.value)}
      >
        {options.map((option) => (
          <option key={option} value={option}>
            {labelize(option)}
          </option>
        ))}
      </select>
    </label>
  );
}

export function NumberField({
  label,
  value,
  min,
  step,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  step: number;
  onChange: (value: number) => void;
}) {
  const normalizedValue = Number.isFinite(value) && value >= min ? value : min;
  const [draft, setDraft] = React.useState(String(normalizedValue));
  const lastValid = React.useRef(normalizedValue);

  React.useEffect(() => {
    const next =
      Number.isFinite(value) && value >= min ? value : lastValid.current;
    lastValid.current = next;
    setDraft(String(next));
  }, [min, value]);

  const commitDraft = (nextDraft: string) => {
    setDraft(nextDraft);
    const parsed = Number(nextDraft);
    if (nextDraft.trim() === "" || !Number.isFinite(parsed) || parsed < min) {
      return;
    }
    lastValid.current = parsed;
    onChange(parsed);
  };

  const restoreLastValid = () => {
    setDraft(String(lastValid.current));
  };

  return (
    <label className="input-field">
      <span>{label}</span>
      <input
        type="number"
        min={min}
        step={step}
        value={draft}
        onBlur={restoreLastValid}
        onChange={(event) => commitDraft(event.currentTarget.value)}
      />
    </label>
  );
}

export function TextField({
  label,
  value,
  type = "text",
  onChange,
}: {
  label: string;
  value: string;
  type?: "text" | "password";
  onChange: (value: string) => void;
}) {
  return (
    <label className="input-field">
      <span>{label}</span>
      <input
        type={type}
        value={value}
        onChange={(event) => onChange(event.currentTarget.value)}
      />
    </label>
  );
}

export function ProviderBlock({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <details className="provider-block" open={title === "llama.cpp"}>
      <summary>{title}</summary>
      <div className="field-grid">{children}</div>
    </details>
  );
}

export function ProviderVisibilityGroup<T extends string>({
  title,
  description,
  options,
  visibility,
  onChange,
}: {
  title: string;
  description: string;
  options: Array<{ value: T; label: string; description: string }>;
  visibility: Record<T, boolean>;
  onChange: (provider: T, enabled: boolean) => void;
}) {
  return (
    <section className="visibility-group">
      <div>
        <h5>{title}</h5>
        <p>{description}</p>
      </div>
      <div className="provider-toggle-grid">
        {options.map((option) => {
          const checked = visibility[option.value];
          const disableLast = checked && enabledCount(visibility) <= 1;
          return (
            <label className="provider-toggle" key={option.value}>
              <span>
                <strong>{option.label}</strong>
                <small>{option.description}</small>
              </span>
              <input
                type="checkbox"
                checked={checked}
                disabled={disableLast}
                onChange={(event) =>
                  onChange(option.value, event.currentTarget.checked)
                }
              />
            </label>
          );
        })}
      </div>
    </section>
  );
}

export function SettingsToggle({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label className="provider-toggle settings-toggle">
      <span>
        <strong>{label}</strong>
      </span>
      <input
        type="checkbox"
        checked={checked}
        onChange={(event) => onChange(event.currentTarget.checked)}
      />
    </label>
  );
}

export function FieldBlock({
  title,
  empty,
  children,
}: {
  title: string;
  empty: boolean;
  children: React.ReactNode;
}) {
  return (
    <section className="field-block">
      <h4>{title}</h4>
      {empty ? (
        <p className="muted">No {title.toLowerCase()} available.</p>
      ) : (
        children
      )}
    </section>
  );
}

export function Banner({
  tone,
  message,
  onClose,
}: {
  tone: "success" | "error";
  message: string;
  onClose: () => void;
}) {
  return (
    <div className={`banner ${tone}`} role="alert">
      {tone === "success" ? (
        <CheckCircle2 size={18} aria-hidden="true" />
      ) : (
        <AlertTriangle size={18} aria-hidden="true" />
      )}
      <span>{message}</span>
      <button onClick={onClose} aria-label="Dismiss">
        x
      </button>
    </div>
  );
}

export function StatusIcon({ status }: { status: DesktopJob["status"] }) {
  if (status === "queued")
    return <Clock className="status-icon queued" size={18} />;
  if (status === "completed")
    return <CheckCircle2 className="status-icon completed" size={18} />;
  if (status === "failed")
    return <AlertTriangle className="status-icon failed" size={18} />;
  if (status === "canceled")
    return <CircleStop className="status-icon canceled" size={18} />;
  return <span className="spinner small" />;
}

export function ReviewCard({
  icon,
  title,
  rows,
}: {
  icon: React.ReactNode;
  title: string;
  rows: Array<[string, boolean | string]>;
}) {
  return (
    <section className="review-card">
      <header>
        {icon}
        <h5>{title}</h5>
      </header>
      <div>
        {rows.map(([label, value]) => (
          <div className="review-row" key={label}>
            <span>{label}</span>
            {typeof value === "boolean" ? (
              <BooleanPill value={value} />
            ) : (
              <strong>{value}</strong>
            )}
          </div>
        ))}
      </div>
    </section>
  );
}

export function BooleanPill({ value }: { value: boolean }) {
  return (
    <span className={`boolean-pill ${value ? "yes" : ""}`}>
      {value ? "Yes" : "No"}
    </span>
  );
}

export function SwitchRow({
  label,
  description,
  checked,
  onChange,
}: {
  label: string;
  description: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label className="switch-row">
      <span>
        <strong>{label}</strong>
        <small>{description}</small>
      </span>
      <input
        type="checkbox"
        checked={checked}
        onChange={(event) => onChange(event.currentTarget.checked)}
      />
    </label>
  );
}

export function ChoiceGroup<T extends string>({
  label,
  description,
  value,
  options,
  availability,
  availabilityRole,
  onChange,
}: {
  label: string;
  description: string;
  value: T;
  options: Array<{ value: T; label: string; description: string }>;
  availability?: ProviderAvailabilityMap;
  availabilityRole?: ProviderAvailabilityRole;
  onChange: (value: T) => void;
}) {
  const labelId = React.useId();
  const descriptionId = React.useId();

  return (
    <div className="choice-group">
      <div>
        <h5 id={labelId}>{label}</h5>
        <p id={descriptionId}>{description}</p>
      </div>
      <div
        className="choice-list"
        role="radiogroup"
        aria-labelledby={labelId}
        aria-describedby={descriptionId}
      >
        {options.map((option) => {
          const selected = value === option.value;
          const providerStatus = availabilityRole
            ? availability?.[
                providerAvailabilityKey(availabilityRole, option.value)
              ]
            : null;
          return (
            <button
              key={option.value}
              type="button"
              role="radio"
              aria-checked={selected}
              className={`choice-card ${selected ? "selected" : ""}`}
              onClick={() => onChange(option.value)}
            >
              <span className="choice-copy">
                <span className="choice-title-row">
                  <strong>{option.label}</strong>
                  {providerStatus && (
                    <ProviderAvailabilityBadge availability={providerStatus} />
                  )}
                </span>
                <small>{option.description}</small>
              </span>
              <span className="radio-mark" />
            </button>
          );
        })}
      </div>
    </div>
  );
}

export function ProviderAvailabilityBadge({
  availability,
}: {
  availability: ProviderAvailability;
}) {
  const label = availability.status === "ready" ? "Ready" : "Offline";
  return (
    <span
      className={`provider-status ${availability.status}`}
      title={availability.message}
    >
      <span className="provider-status-dot" />
      {label}
    </span>
  );
}
