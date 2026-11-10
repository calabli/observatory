// Fetching and raw shape of the models.dev public data (https://models.dev/api.json).
// Price import for usage stats and model parameter completion in the provider
// form share this single data set and query cache.

export const MODELS_DEV_API_URL = "https://models.dev/api.json";

export interface ModelsDevCost {
  input?: number;
  output?: number;
  cache_read?: number;
  cache_write?: number;
}

export interface ModelsDevModalities {
  input?: string[];
  output?: string[];
}

export interface ModelsDevLimit {
  context?: number;
  input?: number;
  output?: number;
}

export interface ModelsDevReasoningOption {
  type?: string;
  values?: string[];
}

export interface ModelsDevModel {
  id?: string;
  name?: string;
  release_date?: string;
  cost?: ModelsDevCost;
  modalities?: ModelsDevModalities;
  limit?: ModelsDevLimit;
  reasoning?: boolean;
  reasoning_options?: ModelsDevReasoningOption[];
  // Reseller / aggregator entries point back to the original vendor entry,
  // e.g. "zhipuai/glm-5".
  canonical_model_id?: string;
  status?: string;
}

export interface ModelsDevProvider {
  id?: string;
  name?: string;
  // Provider API base URL; it is what maps a user-entered address to a vendor.
  api?: string;
  models?: Record<string, ModelsDevModel>;
}

export type ModelsDevResponse = Record<string, ModelsDevProvider>;

export async function fetchModelsDev(): Promise<ModelsDevResponse> {
  const { usageCall } = await import("../native/usageClient");
  return usageCall<ModelsDevResponse>("modelsdev_fetch");
}

/**
 * Strip the namespace prefix (`vendor/`), the `:variant` suffix and the
 * `[1m]` marker, then lowercase, so that the same model spelled differently
 * across providers collapses onto one ID.
 */
export function normalizeModelsDevModelId(modelId: string): string {
  const afterSlash = modelId.slice(modelId.lastIndexOf("/") + 1);
  const beforeColon = afterSlash.split(":")[0] ?? "";
  let normalized = beforeColon.trim().replace(/@/g, "-").toLowerCase();
  if (normalized.endsWith("[1m]")) {
    normalized = normalized.slice(0, -"[1m]".length).trim();
  }
  return normalized;
}
