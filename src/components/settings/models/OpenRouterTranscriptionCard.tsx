import React, { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle, Check, Cloud, KeyRound, RefreshCw } from "lucide-react";
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
 * The OpenRouter transcription source, presented as its own section next to the
 * local model sections.
 *
 * The tile deliberately mirrors the local `ModelCard` anatomy — border and
 * active tint, title + `Active` badge, description, hairline, then a metadata
 * row of icon chips with the actions pushed to the end — so a cloud selection
 * reads as one more model source rather than a separate kind of thing. What the
 * catalog cannot promise (a disk size, accuracy/speed scores, deletion) simply
 * has no chip here.
 *
 * Discovery lives in component state only: it is refetched on mount and on
 * demand, never persisted, so a stale list can't outlive the provider's
 * offerings. A saved model that the latest catalog lacks stays selected and is
 * called out with the warning chip instead of being silently replaced.
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
  // selected-but-missing instead of being replaced. An empty catalog counts:
  // nothing in it can match.
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
          // The selection never committed; show the model that is still saved
          // instead of leaving an uncommitted choice in the picker.
          setDraftModel(null);
        } else if (result.status === "ok") {
          await refreshSettings();
          onActivated?.();
        }
      } catch (error) {
        setActionError(String(error));
        setDraftModel(null);
      } finally {
        setActivating(false);
      }
    },
    [refreshSettings, onActivated],
  );

  const handleModelChange = (value: string | null) => {
    if (!value) return;
    setActionError(null);
    // Show the pending choice immediately — for an active selection the commit
    // round-trips through settings, and the picker must not lag behind the click.
    setDraftModel(value);
    if (isActive) {
      // Switching while active commits immediately; the old selection stays
      // persisted until the backend confirms the new one.
      void activate(value);
    }
  };

  const handleKeyBlur = async (value: string) => {
    setKeyEditing(false);
    await updatePostProcessApiKey(OPENROUTER_PROVIDER_ID, value);
  };

  const keyBusy = isUpdating(`post_process_api_key:${OPENROUTER_PROVIDER_ID}`);
  const canActivate =
    hasKey && selectedModel.trim().length > 0 && !activating && !keyBusy;
  const showKeyField = !hasKey || keyEditing;

  const borderClass = isActive
    ? "border-logo-primary/50 bg-logo-primary/10"
    : "border-mid-gray/20";

  return (
    <div className="space-y-3">
      {/* Section header follows the local sections: label left, icon action
          right (same button as the models-header rescan control). */}
      <div className="flex items-center justify-between">
        <h2 className="text-sm font-medium text-text/60">
          {t("settings.models.openrouter.section")}
        </h2>
        <button
          type="button"
          onClick={() => void loadCatalog()}
          disabled={catalogLoading}
          title={t("settings.models.openrouter.refresh")}
          aria-label={t("settings.models.openrouter.refresh")}
          className="flex items-center justify-center w-8 h-8 text-sm font-medium rounded-lg bg-mid-gray/10 text-text/60 hover:bg-mid-gray/20 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
        >
          <RefreshCw
            className={`w-3.5 h-3.5 ${catalogLoading ? "animate-spin" : ""}`}
          />
        </button>
      </div>

      <div
        className={`flex flex-col rounded-xl px-4 py-3 gap-2 text-left transition-all duration-200 border-2 ${borderClass}`}
      >
        {/* Title row: name + state badge, then the description — same as a
            local model card. */}
        <div className="flex justify-between items-center w-full">
          <div className="flex flex-col items-start flex-1 min-w-0">
            <div className="flex items-center gap-3 flex-wrap">
              {/* eslint-disable-next-line i18next/no-literal-string -- brand name */}
              <h3 className="text-base font-semibold text-text">OpenRouter</h3>
              {isActive && (
                <Badge variant="primary">
                  <Check className="w-3 h-3 mr-1" />
                  {t("modelSelector.active")}
                </Badge>
              )}
            </div>
            <p className="text-text/60 text-sm leading-relaxed">
              {t("settings.models.openrouter.description")}
            </p>
          </div>
        </div>

        <hr className="w-full border-mid-gray/20" />

        {/* Model picker — searchable, and able to show a saved model that the
            current catalog no longer contains. Compact control: this card
            stacks a picker, a key field and a metadata row. */}
        <Select
          value={selectedModel || null}
          options={options}
          onChange={handleModelChange}
          placeholder={t("settings.models.openrouter.modelPlaceholder")}
          isClearable={false}
          isLoading={catalogLoading}
          disabled={activating}
          size="sm"
        />

        {!catalogError && !catalogLoading && catalogLoaded && catalog.length === 0 && (
          <p className="text-xs text-text/40">
            {t("settings.models.openrouter.noModels")}
          </p>
        )}

        {showKeyField && (
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium text-text/60">
              {t("settings.models.openrouter.apiKeyLabel")}
            </label>
            <ApiKeyField
              value={apiKey}
              onBlur={(value) => void handleKeyBlur(value)}
              disabled={keyBusy}
              placeholder={t("settings.models.openrouter.apiKeyPlaceholder")}
              className="!min-w-0 w-full"
            />
          </div>
        )}

        {/* Metadata chips + actions, matching a local card's bottom row. */}
        <div className="flex items-center gap-3 w-full -mb-0.5 mt-0.5 h-5">
          {hasKey ? (
            <div
              className="flex items-center gap-1 text-xs text-text/50"
              title={t("settings.models.openrouter.keyConfigured")}
            >
              <Check className="w-3.5 h-3.5 text-green-500" />
              <span>{t("settings.models.openrouter.keyConfigured")}</span>
            </div>
          ) : (
            <div
              className="flex items-center gap-1 text-xs text-text/50"
              title={t("settings.models.openrouter.apiKeyLabel")}
            >
              <KeyRound className="w-3.5 h-3.5" />
              <span>{t("settings.models.openrouter.keyMissing")}</span>
            </div>
          )}

          {savedMissingFromCatalog && (
            <div
              className="flex items-center gap-1 text-xs text-text/50"
              title={t("settings.models.openrouter.notInCatalog")}
            >
              <AlertTriangle className="w-3.5 h-3.5 text-orange-400" />
              <span className="max-w-[12rem] truncate">
                {t("settings.models.openrouter.notInCatalog")}
              </span>
            </div>
          )}

          {hasKey && !keyEditing && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setKeyEditing(true)}
              className="flex items-center gap-1.5 ms-auto text-text/50 hover:text-text/80"
            >
              <KeyRound className="w-3.5 h-3.5" />
              <span>{t("settings.models.openrouter.changeKey")}</span>
            </Button>
          )}

          {!isActive && (
            <Button
              variant="ghost"
              size="sm"
              disabled={!canActivate}
              onClick={() => void activate(selectedModel)}
              title={t("settings.models.openrouter.use")}
              className={`flex items-center gap-1.5 text-logo-primary/85 hover:text-logo-primary hover:bg-logo-primary/10 ${
                hasKey && !keyEditing ? "" : "ms-auto"
              }`}
            >
              <Cloud className="w-3.5 h-3.5" />
              <span>
                {activating
                  ? t("settings.models.openrouter.activating")
                  : t("settings.models.openrouter.use")}
              </span>
            </Button>
          )}
        </div>

        {catalogError && (
          <p className="text-xs text-error break-all" aria-live="polite">
            {catalogError}
          </p>
        )}
        {actionError && (
          <p className="text-xs text-error break-all" aria-live="polite">
            {actionError}
          </p>
        )}

        <p className="text-xs text-text/40">
          {t("settings.models.openrouter.privacyNote")}
        </p>
      </div>
    </div>
  );
};

export default OpenRouterTranscriptionCard;
