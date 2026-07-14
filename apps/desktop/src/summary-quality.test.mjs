import assert from "node:assert/strict";
import test from "node:test";
import { isSummaryUnvalidated } from "./summary-quality.ts";

test("unvalidated requires notes and a numeric relevancy judgment", () => {
  assert.equal(
    isSummaryUnvalidated({
      summary_notes: ["Legacy CLI note"],
      summary_relevancy: null,
      summary_quality_validated: false,
    }),
    false,
  );
  assert.equal(
    isSummaryUnvalidated({
      summary_notes: ["HTTP summary note"],
      summary_relevancy: 0,
      summary_quality_validated: false,
    }),
    true,
  );
  assert.equal(
    isSummaryUnvalidated({
      summary_notes: ["Validated note"],
      summary_relevancy: 90,
      summary_quality_validated: true,
    }),
    false,
  );
  assert.equal(
    isSummaryUnvalidated({
      summary_notes: [],
      summary_relevancy: 50,
      summary_quality_validated: false,
    }),
    false,
  );
});
