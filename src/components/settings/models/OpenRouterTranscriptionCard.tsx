import React, { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, RefreshCw } from "lucide-react";
import { commands, type Result } from "@/bindings";
import Badge from "@/components/ui/Badge";
import { Button } from "@/components/ui/Button";
import { Select, type SelectOption } from "@/components/ui/Select";
import { useSettings } from "@/hooks/useSettings";
import { ApiKeyField } from "../PostProcessingSettingsApi/ApiKeyField";

/** Provider id owning the shared OpenRouter API key (see settings.rs). */
const OPENROUTER_PROVIDER_ID = "openrouter";

interface OpenRouterTranscriptionCardProps {
  /**
   * Called after the cloud selection becomes active. Onboarding uses it to
   * advance without requiring a local model download.
   */
  onActivated?: () => void;
}

/**
 * In-flight catalog request, shared by every tile instance. Concurrent opens
 * (e.g. onboarding and the Models page mounting together) join one request
 * instead of each hitting the public catalog endpoint.
 */
let catalogRequest: Promise<Result<string[], string>> | null = null;

const fetchCatalog = () => {
  if (!catalogRequest) {
    catalogRequest = commands
      .fetchOpenrouterTranscriptionModels()
      .finally(() => {
        catalogRequest = null;
      });
  }
  return catalogRequest;
};

/**
 * The single OpenRouter transcription tile on the Models page.
 *
 * It offers the two things that only belong here — a searchable picker over
 * OpenRouter's *current* speech-to-text catalog and the shared API key — plus one
 * action to make the selection active. The catalog deliberately lives in
 * component state: it is refetched on mount and on demand, never persisted, so a
 * stale list can't be shown after the provider changes its offerings.
 */
export const OpenRouterTranscriptionCard: React.FC<
  OpenRouterTranscriptionCardProps
> = ({ onActivated }) => {
  const { t } = useTranslation();
  const { settings, refreshSettings, updatePostProcessApiKey, isUpdating } =
    useSettings();

  const [catalog, setCatalog] = useState<string[]>([]);
  const [catalogLoading, setCatalogLoading] = useState(false);
  const [catalogError, setCatalogError] = useState<string | null>(null);
  /** True once a catalog request has completed successfully. */
  const [catalogLoaded, setCatalogLoaded] = useState(false);
  const [draftModel, setDraftModel] = useState<string | null>(null);
  const [keyEditing, setKeyEditing] = useState(false);
  const [activating, setActivating] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  const savedModel = settings?.openrouter_transcription_model ?? "";
  const apiKey = settings?.post_process_api_keys?.[OPENROUTER_PROVIDER_ID] ?? "";
  const hasKey = apiKey.trim().length > 0;
  const isActive = settings?.transcription_provider === "openrouter";

  // The picker shows the pending choice while one exists, otherwise whatever is
  // saved — including a model that discovery no longer lists (it stays usable).
  const selectedModel = draftModel ?? savedModel;

  // A committed selection clears the draft so the picker follows the backend.
  useEffect(() => {
    setDraftModel(null);
  }, [savedModel, isActive]);

  const loadCatalog = useCallback(async () => {
    setCatalogLoading(true);
    setCatalogError(null);
    try {
      const result = await fetchCatalog();
      if (result.status === "ok") {
        setCatalog(result.data);
        setCatalogLoaded(true);
      } else {
        // Keep the previously fetched options visible; the error explains why
        // the list could not be refreshed.
        setCatalogError(result.error);
      }
    } catch (error) {
      setCatalogError(String(error));
    } finally {
      setCatalogLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadCatalog();
  }, [loadCatalog]);

  const options = useMemo<SelectOption[]>(
    () => catalog.map((id) => ({ value: id, label: id })),
    [catalog],
  );

  // A saved selection the last successful catalog does not contain is shown as
  // selected-but-missing instead of being silently replaced. An empty catalog
  // counts: nothing in it can match.
  const savedMissingFromCatalog =
    catalogLoaded &&
    selectedModel.trim().length > 0 &&
    !catalog.includes(selectedModel);

  const activate = useCallback(
    async (modelId: string) => {
      setActivating(true);
      setActionError(null);
      try {
        const result =
          await commands.selectOpenrouterTranscriptionModel(modelId);
        if (result.status === "error") {
          setActionError(result.error);
        } else if (result.status === "ok") {
          await refreshSettings();
          onActivated?.();
        }
      } catch (error) {
        setActionError(String(error));
      } finally {
        setActivating(false);
      }
    },
    [refreshSettings, onActivated],
  );

  const handleModelChange = (value: string | null) => {
    if (!value) return;
    setActionError(null);
    if (isActive) {
      // Switching while active commits immediately; the old selection stays
      // persisted until the backend confirms the new one.
      void activate(value);
    } else {
      setDraftModel(value);
    }
  };

  const handleKeyBlur = async (value: string) => {
    setKeyEditing(false);
    await updatePostProcessApiKey(OPENROUTER_PROVIDER_ID, value);
  };

  const keyBusy = isUpdating(`post_process_api_key:${OPENROUTER_PROVIDER_ID}`);
  const canActivate =
    hasKey && selectedModel.trim().length > 0 && !activating && !keyBusy;

  const borderClass = isActive
    ? "border-logo-primary/50 bg-logo-primary/10"
    : "border-mid-gray/20";

  return (
    <div
      className={`flex flex-col rounded-xl px-4 py-3 gap-2 border-2 transition-all duration-200 ${borderClass}`}
    >
      <div className="flex items-start justify-between w-full gap-3">
        <div className="flex flex-col items-start flex-1 min-w-0">
          <div className="flex items-center gap-3 flex-wrap">
            <h3 className="text-base font-semibold text-text">
              {t("settings.models.openrouter.title")}
            </h3>
            {isActive && (
              <Badge variant="primary">{t("modelSelector.active")}</Badge>
            )}
          </div>
          <p className="text-sm text-text/60 leading-relaxed">
            {t("settings.models.openrouter.description")}
          </p>
        </div>

        {!isActive && (
          <Button
            variant="primary"
            size="sm"
            className="shrink-0"
            disabled={!canActivate}
            onClick={() => void activate(selectedModel)}
          >
            {activating
              ? t("settings.models.openrouter.activating")
              : t("settings.models.openrouter.use")}
          </Button>
        )}
      </div>

      <hr className="w-full border-mid-gray/20" />

      {/* Model picker — searchable, and able to show a saved model that the
          current catalog no longer contains. */}
      <div className="flex items-center gap-2">
        <div className="flex-1 min-w-0">
          <Select
            value={selectedModel || null}
            options={options}
            onChange={handleModelChange}
            placeholder={t("settings.models.openrouter.modelPlaceholder")}
            isClearable={false}
            isLoading={catalogLoading}
            disabled={activating}
          />
        </div>
        <Button
          variant="secondary"
          size="sm"
          className="shrink-0 justify-center"
          onClick={() => void loadCatalog()}
          disabled={catalogLoading}
          title={t("settings.models.openrouter.refresh")}
          aria-label={t("settings.models.openrouter.refresh")}
        >
          <RefreshCw
            className={`w-3.5 h-3.5 ${catalogLoading ? "animate-spin" : ""}`}
          />
        </Button>
      </div>

      {catalogError && (
        <p className="text-xs text-red-400 break-all">{catalogError}</p>
      )}
      {!catalogError && savedMissingFromCatalog && (
        <p className="text-xs text-text/40">
          {t("settings.models.openrouter.notInCatalog")}
        </p>
      )}
      {!catalogError &&
        !savedMissingFromCatalog &&
        catalogLoaded &&
        catalog.length === 0 && (
          <p className="text-xs text-text/40">
            {t("settings.models.openrouter.noModels")}
          </p>
        )}

      {/* Shared credential: the same key the post-processing pipeline uses. */}
      {hasKey && !keyEditing ? (
        <button
          type="button"
          onClick={() => setKeyEditing(true)}
          className="flex items-center gap-1.5 text-xs text-text/50 hover:text-text/80 transition-colors w-fit"
        >
          <Check className="w-3 h-3 text-green-500" />
          <span>{t("settings.models.openrouter.keyConfigured")}</span>
          <span className="text-text/30">·</span>
          <span className="underline">
            {t("settings.models.openrouter.changeKey")}
          </span>
        </button>
      ) : (
        <div className="flex flex-col gap-1">
          <label className="text-xs font-medium text-text/60">
            {t("settings.models.openrouter.apiKeyLabel")}
          </label>
          <ApiKeyField
            value={apiKey}
            onBlur={(value) => void handleKeyBlur(value)}
            disabled={keyBusy}
            placeholder={t("settings.models.openrouter.apiKeyPlaceholder")}
            className="!min-w-0"
          />
        </div>
      )}

      {actionError && (
        <p className="text-xs text-red-400 break-all">{actionError}</p>
      )}

      <p className="text-xs text-text/30">
        {t("settings.models.openrouter.privacyNote")}
      </p>
    </div>
  );
};

export default OpenRouterTranscriptionCard;
