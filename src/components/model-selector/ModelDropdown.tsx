import React from "react";
import { useTranslation } from "react-i18next";
import type { ModelInfo } from "@/bindings";
import {
  getTranslatedModelName,
  getTranslatedModelDescription,
} from "../../lib/utils/modelTranslation";

interface ModelDropdownProps {
  models: ModelInfo[];
  currentModelId: string;
  /** True while OpenRouter is the active backend (configured or not). */
  isCloud: boolean;
  /** True when the saved OpenRouter selection has a key and a model. */
  cloudConfigured: boolean;
  /** The remembered OpenRouter model id ("" until one is chosen). */
  cloudModel: string;
  onModelSelect: (modelId: string) => void;
  /** OpenRouter row: activate the saved selection, or open Models to pick one. */
  onCloudSelect: () => void;
}

const ModelDropdown: React.FC<ModelDropdownProps> = ({
  models,
  currentModelId,
  isCloud,
  cloudConfigured,
  cloudModel,
  onModelSelect,
  onCloudSelect,
}) => {
  const { t } = useTranslation();
  const downloadedModels = models.filter((m) => m.is_downloaded);

  const handleModelClick = (modelId: string) => {
    onModelSelect(modelId);
  };

  const handleRowKeyDown = (event: React.KeyboardEvent, action: () => void) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      action();
    }
  };

  return (
    <div className="absolute bottom-full start-0 mb-2 w-64 max-h-[80vh] overflow-y-auto bg-background border border-mid-gray/20 rounded-lg shadow-lg py-2 z-50">
      {downloadedModels.length > 0 ? (
        <div>
          {downloadedModels.map((model) => (
            <div
              key={model.id}
              onClick={() => handleModelClick(model.id)}
              onKeyDown={(e) => handleRowKeyDown(e, () => handleModelClick(model.id))}
              tabIndex={0}
              role="button"
              className={`w-full px-3 py-2 text-start hover:bg-mid-gray/10 transition-colors cursor-pointer focus:outline-none ${
                !isCloud && currentModelId === model.id
                  ? "bg-logo-primary/10 text-logo-primary"
                  : ""
              }`}
            >
              <div className="flex items-center justify-between">
                <div>
                  <div className="text-sm text-text/80">
                    {getTranslatedModelName(model, t)}
                    {model.is_custom && (
                      <span className="ms-1.5 text-[10px] font-medium text-text/40 uppercase">
                        {t("modelSelector.custom")}
                      </span>
                    )}
                    {model.supports_streaming && (
                      <span className="ms-1.5 text-[10px] font-medium text-logo-primary/70 uppercase">
                        {t("modelSelector.streaming")}
                      </span>
                    )}
                  </div>
                  <div className="text-xs text-text/40 italic pe-4">
                    {getTranslatedModelDescription(model, t)}
                  </div>
                </div>
                {!isCloud && currentModelId === model.id && (
                  <div className="text-xs text-logo-primary">
                    {t("modelSelector.active")}
                  </div>
                )}
              </div>
            </div>
          ))}
        </div>
      ) : (
        <div className="px-3 py-2 text-sm text-text/60">
          {t("modelSelector.noModelsAvailable")}
        </div>
      )}

      {/* One OpenRouter row: a source switch, not a second catalog. Activating
          or picking a model happens on the Models page. */}
      <div className="border-t border-mid-gray/20 my-1" />
      <div
        onClick={onCloudSelect}
        onKeyDown={(e) => handleRowKeyDown(e, onCloudSelect)}
        tabIndex={0}
        role="button"
        className={`w-full px-3 py-2 text-start hover:bg-mid-gray/10 transition-colors cursor-pointer focus:outline-none ${
          isCloud ? "bg-logo-primary/10 text-logo-primary" : ""
        }`}
      >
        <div className="flex items-center justify-between gap-2">
          <div className="min-w-0">
            {/* eslint-disable-next-line i18next/no-literal-string -- brand name */}
            <div className="text-sm text-text/80">OpenRouter</div>
            <div
              className="text-xs text-text/40 italic truncate"
              title={cloudModel || undefined}
            >
              {cloudModel || t("modelSelector.openrouterNotConfigured")}
            </div>
          </div>
          {isCloud ? (
            <div className="text-xs text-logo-primary shrink-0">
              {t("modelSelector.active")}
            </div>
          ) : (
            <div className="text-xs text-text/40 shrink-0">
              {cloudConfigured
                ? t("modelSelector.use")
                : t("modelSelector.configure")}
            </div>
          )}
        </div>
      </div>
    </div>
  );
};

export default ModelDropdown;
