export function formatMetricModel(model?: string | null): string {
  const resolved = model?.trim();
  return resolved || "CLI default";
}
