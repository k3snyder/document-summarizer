export interface SummaryQualityFields {
  summary_notes?: string[] | null;
  summary_relevancy?: number | null;
  summary_quality_validated?: boolean | null;
}

export function isSummaryUnvalidated(page: SummaryQualityFields): boolean {
  return (
    page.summary_quality_validated === false &&
    typeof page.summary_relevancy === "number" &&
    !!page.summary_notes?.length
  );
}
