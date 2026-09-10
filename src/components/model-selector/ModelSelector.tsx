import React, { useState, useRef, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { commands } from "@/bindings";
import { getTranslatedModelName } from "../../lib/utils/modelTranslation";
import { useModelStore } from "../../stores/modelStore";
import { useSettings } from "../../hooks/useSettings";
import ModelStatusButton from "./ModelStatusButton";
import ModelDropdown from "./ModelDropdown";
import DownloadProgressDisplay from "./DownloadProgressDisplay";

import { ModelStateEvent } from "@/lib/types/events";

type ModelStatus =
  | "ready"
  | "loading"
  | "downloading"
  | "verifying"
  | "extracting"
  | "error"
  | "unloaded"
  | "none";

interface ModelSelectorProps {
  onError?: (error: string) => void;
  /** Open the Models settings section (for first-time OpenRouter setup). */
  onOpenModels?: () => void;
}

const ModelSelector: React.FC<ModelSelectorProps> = ({
  onError,
  onOpenModels,
}) => {
  const { t } = useTranslation();
  const {
    models,
    currentModel,
    downloadProgress,
    downloadStats,
    verifyingModels,
    extractingModels,
    selectModel,
  } = useModelStore();
  const { settings } = useSettings();

  // OpenRouter is a separate backend: while it is active nothing local is
  // loaded, so its status must never be reported as a local load state.
  const cloudModel = settings?.openrouter_transcription_model ?? "";
  const isCloud = settings?.transcription_provider === "openrouter";
  const cloudConfigured =
    isCloud &&
    cloudModel.trim().length > 0 &&
    (settings?.post_process_api_keys?.openrouter ?? "").trim().length > 0;

  const [modelStatus, setModelStatus] = useState<ModelStatus>("unloaded");
  const [modelError, setModelError] = useState<string | null>(null);
  const [showModelDropdown, setShowModelDropdown] = useState(false);
  // Track pending model switch for optimistic display
  const [pendingModelId, setPendingModelId] = useState<string | null>(null);

  const dropdownRef = useRef<HTMLDivElement>(null);

  const displayModelId = pendingModelId || currentModel;

  // Check model status when currentModel changes
  useEffect(() => {
    const checkStatus = async () => {
      if (isCloud) {
        // The cloud selection has no local engine to report on; the render path
        // derives its own status.
        return;
      }
      if (currentModel) {
        try {
          const statusResult = await commands.getTranscriptionModelStatus();
          if (statusResult.status === "ok") {
            setModelStatus(
              statusResult.data === currentModel ? "ready" : "unloaded",
            );
          }
        } catch {
          setModelStatus("error");
          setModelError("Failed to check model status");
        }
      } else {
        setModelStatus("none");
      }
    };
    checkStatus();
  }, [currentModel, isCloud]);

  useEffect(() => {
    // Listen for model loading lifecycle events
    const modelStateUnlisten = listen<ModelStateEvent>(
      "model-state-changed",
      (event) => {
        const { event_type, error } = event.payload;
        switch (event_type) {
          case "loading_started":
            setModelStatus("loading");
            setModelError(null);
            break;
          case "loading_completed":
            setModelStatus("ready");
            setModelError(null);
            setPendingModelId(null);
            break;
          case "loading_failed":
            setModelStatus("error");
            setModelError(error || "Failed to load model");
            setPendingModelId(null);
            break;
          case "unloaded":
            setModelStatus("unloaded");
            setModelError(null);
            break;
        }
      },
    );

    // Auto-select model when download completes (fires after extraction too)
    const downloadCompleteUnlisten = listen<string>(
      "model-download-complete",
      (event) => {
        // A finished download never changes the active backend: while OpenRouter
        // is running, the cloud selection stays.
        if (isCloud) return;
        const modelId = event.payload;
        setTimeout(async () => {
          try {
            const isRecording = await commands.isRecording();
            if (!isRecording) {
              setPendingModelId(modelId);
              setModelError(null);
              setShowModelDropdown(false);
              const success = await selectModel(modelId);
              if (!success) {
                setPendingModelId(null);
              }
            }
          } catch {
            // Ignore errors in auto-select
          }
        }, 500);
      },
    );

    // Click outside to close dropdown
    const handleClickOutside = (event: MouseEvent) => {
      if (
        dropdownRef.current &&
        !dropdownRef.current.contains(event.target as Node)
      ) {
        setShowModelDropdown(false);
      }
    };

    document.addEventListener("mousedown", handleClickOutside);

    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
      modelStateUnlisten.then((fn) => fn());
      downloadCompleteUnlisten.then((fn) => fn());
    };
  }, [selectModel, isCloud]);

  const handleModelSelect = async (modelId: string) => {
    setPendingModelId(modelId);
    setModelError(null);
    setShowModelDropdown(false);
    const success = await selectModel(modelId);
    if (!success) {
      setPendingModelId(null);
      setModelStatus("error");
      setModelError("Failed to switch model");
      onError?.("Failed to switch model");
    }
  };

  /**
   * OpenRouter row: activate the saved cloud selection when it is ready, and
   * otherwise send the user to the Models page where a model can be chosen.
   */
  const handleCloudSelect = async () => {
    setShowModelDropdown(false);
    if (isCloud || !cloudConfigured) {
      onOpenModels?.();
      return;
    }

    setModelError(null);
    const result =
      await commands.selectOpenrouterTranscriptionModel(cloudModel);
    if (result.status === "error") {
      setModelError(result.error);
      onError?.(result.error);
    }
  };

  const getModelDisplayText = (): string => {
    if (isCloud) {
      // Cloud shows the saved model rather than any local load state, and is
      // never presented as online/healthy from configuration alone.
      return cloudModel.trim().length > 0
        ? t("modelSelector.openrouter", { model: cloudModel })
        : t("modelSelector.openrouterNotConfigured");
    }

    const verifyingKeys = Object.keys(verifyingModels);
    if (verifyingKeys.length > 0) {
      if (verifyingKeys.length === 1) {
        const modelId = verifyingKeys[0];
        const model = models.find((m) => m.id === modelId);
        const modelName = model
          ? getTranslatedModelName(model, t)
          : t("modelSelector.verifyingGeneric").replace("...", "");
        return t("modelSelector.verifying", { modelName });
      } else {
        return t("modelSelector.verifyingGeneric");
      }
    }

    const extractingKeys = Object.keys(extractingModels);
    if (extractingKeys.length > 0) {
      if (extractingKeys.length === 1) {
        const modelId = extractingKeys[0];
        const model = models.find((m) => m.id === modelId);
        const modelName = model
          ? getTranslatedModelName(model, t)
          : t("modelSelector.extractingGeneric").replace("...", "");
        return t("modelSelector.extracting", { modelName });
      } else {
        return t("modelSelector.extractingMultiple", {
          count: extractingKeys.length,
        });
      }
    }

    const progressValues = Object.values(downloadProgress);
    if (progressValues.length > 0) {
      if (progressValues.length === 1) {
        const progress = progressValues[0];
        const percentage = Math.max(
          0,
          Math.min(100, Math.round(progress.percentage)),
        );
        return t("modelSelector.downloading", { percentage });
      } else {
        return t("modelSelector.downloadingMultiple", {
          count: progressValues.length,
        });
      }
    }

    const currentModelInfo = models.find((m) => m.id === displayModelId);

    switch (modelStatus) {
      case "ready":
        return currentModelInfo
          ? getTranslatedModelName(currentModelInfo, t)
          : t("modelSelector.modelReady");
      case "loading":
        return currentModelInfo
          ? t("modelSelector.loading", {
              modelName: getTranslatedModelName(currentModelInfo, t),
            })
          : t("modelSelector.loadingGeneric");
      case "extracting":
        return currentModelInfo
          ? t("modelSelector.extracting", {
              modelName: getTranslatedModelName(currentModelInfo, t),
            })
          : t("modelSelector.extractingGeneric");
      case "error":
        return modelError || t("modelSelector.modelError");
      case "unloaded":
        return currentModelInfo
          ? getTranslatedModelName(currentModelInfo, t)
          : t("modelSelector.modelUnloaded");
      case "none":
        return t("modelSelector.noModelDownloadRequired");
      default:
        return currentModelInfo
          ? getTranslatedModelName(currentModelInfo, t)
          : t("modelSelector.modelUnloaded");
    }
  };

  // Derive display status from model status + store state
  const getDisplayStatus = (): ModelStatus => {
    if (isCloud) {
      // Neutral while configured, error-coloured only while unusable — never
      // the local "ready" dot, which would claim a loaded local engine.
      return cloudConfigured ? "unloaded" : "none";
    }
    if (Object.keys(verifyingModels).length > 0) return "verifying";
    if (Object.keys(extractingModels).length > 0) return "extracting";
    if (Object.keys(downloadProgress).length > 0) return "downloading";
    return modelStatus;
  };

  return (
    <>
      {/* Model Status and Switcher */}
      <div className="relative" ref={dropdownRef}>
        <ModelStatusButton
          status={getDisplayStatus()}
          displayText={getModelDisplayText()}
          isDropdownOpen={showModelDropdown}
          onClick={() => setShowModelDropdown(!showModelDropdown)}
        />

        {/* Model Dropdown */}
        {showModelDropdown && (
          <ModelDropdown
            models={models}
            currentModelId={displayModelId}
            isCloud={isCloud}
            cloudConfigured={cloudConfigured}
            cloudModel={cloudModel}
            onModelSelect={handleModelSelect}
            onCloudSelect={() => void handleCloudSelect()}
          />
        )}
      </div>

      {/* Download Progress Bar for Models */}
      <DownloadProgressDisplay
        downloadProgress={downloadProgress}
        downloadStats={downloadStats}
      />
    </>
  );
};

export default ModelSelector;
