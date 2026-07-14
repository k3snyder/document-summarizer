import assert from "node:assert/strict";
import test from "node:test";
import { formatMetricModel } from "./metric-model.ts";

test("formats persisted models and falls back for legacy outputs", () => {
  assert.equal(formatMetricModel("gpt-5.6-luna"), "gpt-5.6-luna");
  assert.equal(formatMetricModel(undefined), "CLI default");
  assert.equal(formatMetricModel(null), "CLI default");
  assert.equal(formatMetricModel("   "), "CLI default");
});
