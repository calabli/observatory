import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import ts from "typescript";

const source = await readFile(
  new URL("../src/benchmarks/benchmarksShared.ts", import.meta.url),
  "utf8",
);
const code = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;

const {
  blendedPrice1m,
  computeParetoFrontier,
  normalizeModelSlug,
  baseModelSlug,
  matchLocalUsage,
  parseModelVariant,
  groupModelFamilies,
} = await import(
  "data:text/javascript;base64," + Buffer.from(code).toString("base64")
);

test("blendedPrice1m uses 3:1 input/output weighting with fallbacks", () => {
  assert.equal(blendedPrice1m({ priceInput1m: 4, priceOutput1m: 20 }), 8);
  assert.equal(blendedPrice1m({ priceInput1m: 2, priceOutput1m: null }), 2);
  assert.equal(blendedPrice1m({ priceInput1m: null, priceOutput1m: 12 }), 12);
  assert.equal(
    blendedPrice1m({ priceInput1m: null, priceOutput1m: null }),
    null,
  );
});

test("computeParetoFrontier finds non-dominated models for price and speed", () => {
  const pts = [
    { id: "cheap-weak", x: 1, y: 30 },
    { id: "mid-dominated", x: 5, y: 28 },
    { id: "mid-good", x: 5, y: 50 },
    { id: "expensive-best", x: 20, y: 59 },
    { id: "expensive-worse", x: 25, y: 55 },
  ];
  assert.deepEqual(
    computeParetoFrontier(pts, "price").map((p) => p.id),
    ["cheap-weak", "mid-good", "expensive-best"],
  );

  const speedPts = [
    { id: "slow-smart", x: 40, y: 59 },
    { id: "mid-dominated", x: 80, y: 40 },
    { id: "fast-smart", x: 110, y: 55 },
    { id: "fastest-ok", x: 200, y: 42 },
  ];
  assert.deepEqual(
    computeParetoFrontier(speedPts, "speed").map((p) => p.id),
    ["slow-smart", "fast-smart", "fastest-ok"],
  );
});

test("normalizeModelSlug and matchLocalUsage correlate local model IDs cleanly", () => {
  assert.equal(
    normalizeModelSlug("anthropic/claude-opus-5-5-20260901[1m]"),
    "claude-opus-5-5",
  );
  assert.equal(baseModelSlug("claude-opus-5-5-max"), "claude-opus-5-5");

  const benchmarks = [
    {
      id: "bm-opus-low",
      name: "Claude Opus 5.5 (Low)",
      slug: "claude-opus-5-5-low",
      creatorName: "Anthropic",
      intelligenceIndex: 48.0,
    },
    {
      id: "bm-opus-max",
      name: "Claude Opus 5.5 (Max)",
      slug: "claude-opus-5-5-max",
      creatorName: "Anthropic",
      intelligenceIndex: 59.1,
    },
    {
      id: "bm-gemini",
      name: "Gemini 4 Carbon (Medium)",
      slug: "gemini-4-carbon",
      creatorName: "Google",
      intelligenceIndex: 58.3,
    },
  ];

  const localModels = [
    {
      model: "claude-opus-5-5-20261001",
      requestCount: 42,
      totalTokens: 120000,
      totalCost: "12.50",
      avgCostPerRequest: "0.30",
      successRate: 1,
      speedOutputTokens: 10000,
      speedGenerationMs: 100000,
      firstTokenMedianMs: 1800,
    },
    {
      model: "google/gemini-4-carbon",
      requestCount: 15,
      totalTokens: 50000,
      totalCost: "3.20",
      avgCostPerRequest: "0.21",
      successRate: 1,
    },
  ];

  const matched = matchLocalUsage(benchmarks, localModels);
  assert.equal(matched.size, 2);
  const opus = matched.get("bm-opus-max");
  assert.ok(opus, "prefers highest-scoring variant for base slug match");
  assert.equal(opus.requests, 42);
  assert.equal(opus.totalCostUsd, 12.5);
  assert.equal(opus.localSpeedTps, 100);
  assert.equal(opus.localTtftS, 1.8);

  const gemini = matched.get("bm-gemini");
  assert.ok(gemini);
  assert.equal(gemini.requests, 15);
});

test("parseModelVariant and groupModelFamilies expose all effort tiers cleanly", () => {
  const maxFallback = parseModelVariant({
    name: "Claude Fable 5.2 (Max, Default Fallback)",
    creatorName: "Anthropic",
  });
  assert.equal(maxFallback.familyName, "Claude Fable 5.2");
  assert.equal(maxFallback.tierKey, "max");
  assert.equal(maxFallback.tierLabel, "Max");
  assert.equal(maxFallback.extraNote, "Default Fallback");

  const nonReasoningHigh = parseModelVariant({
    name: "Claude Opus 4.6 (Non-reasoning, High)",
    creatorName: "Anthropic",
  });
  assert.equal(nonReasoningHigh.familyName, "Claude Opus 4.6");
  assert.equal(nonReasoningHigh.tierKey, "non-reasoning");
  assert.equal(nonReasoningHigh.tierLabel, "Non-reasoning · High");

  const dated = parseModelVariant({
    name: "GPT-4o (Dec '24)",
    creatorName: "OpenAI",
  });
  assert.equal(dated.familyName, "GPT-4o (Dec '24)");
  assert.equal(dated.tierKey, "standard");

  const families = groupModelFamilies(
    [
      {
        id: "1",
        name: "Claude Opus 5 (Low)",
        slug: "claude-opus-5-low",
        creatorName: "Anthropic",
        intelligenceIndex: null,
      },
      {
        id: "2",
        name: "Claude Opus 5 (Max)",
        slug: "claude-opus-5-max",
        creatorName: "Anthropic",
        intelligenceIndex: 44.2,
      },
      {
        id: "3",
        name: "Claude Opus 5 (High)",
        slug: "claude-opus-5-high",
        creatorName: "Anthropic",
        intelligenceIndex: null,
      },
      {
        id: "4",
        name: "Claude Opus 5 (Medium)",
        slug: "claude-opus-5-medium",
        creatorName: "Anthropic",
        intelligenceIndex: null,
      },
    ],
    "intelligenceIndex",
  );
  assert.equal(families.length, 1);
  assert.equal(families[0].familyName, "Claude Opus 5");
  assert.equal(families[0].bestScore, 44.2);
  assert.deepEqual(
    families[0].variants.map((v) => v.parsed.tierKey),
    ["max", "high", "medium", "low"],
  );
});
