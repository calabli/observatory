import { usageCall } from "../../native/usageClient";
import type {
  ModelPricing,
  ModelsDevSyncConfig,
  ModelsDevSyncState,
} from "../../types/usage";
export const usageApi = {
  updateModelPricingBatch: (entries: ModelPricing[]) =>
    usageCall<number>("pricing_batch", { entries }),
  getModelsDevSyncConfig: () =>
    usageCall<ModelsDevSyncState>("modelsdev_config_get"),
  saveModelsDevSyncConfig: (config: ModelsDevSyncConfig) =>
    usageCall<ModelsDevSyncState>("modelsdev_config_save", { config }),
  recordModelsDevSyncResult: (syncedAt: number | null, error: string | null) =>
    usageCall<boolean>("modelsdev_record_result", { syncedAt, error }),
};
