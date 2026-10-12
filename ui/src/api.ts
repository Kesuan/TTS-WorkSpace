// Tauri 命令封装：统一错误模型（与 CLI 错误对象同构）+ 音频路径转换。

import { Channel, convertFileSrc, invoke } from "@tauri-apps/api/core";

import type {
  AppInfo,
  BatchEvent,
  BatchResult,
  ConfigPayload,
  DoctorReport,
  ExportResult,
  GcResult,
  ImportOutcome,
  ProjectDetail,
  ProjectList,
  UsageResult,
  VoiceList,
  WorkspaceConfig,
} from "./types";

export class TtsCommandError extends Error {
  code: string;
  hint: string | null;
  details: unknown;

  constructor(raw: { code?: string; message?: string; hint?: string | null; details?: unknown }) {
    const message = raw?.message ?? "未知错误";
    super(raw?.hint ? `${message}；${raw.hint}` : message);
    this.name = "TtsCommandError";
    this.code = raw?.code ?? "E_UNKNOWN";
    this.hint = raw?.hint ?? null;
    this.details = raw?.details ?? null;
  }
}

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function call<T>(
  command: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  if (!isTauriRuntime()) {
    throw new TtsCommandError({
      code: "E_NOT_TAURI",
      message: "当前不在桌面应用中运行",
      hint: "请使用 `tools/gui`（或 cargo tauri dev）启动 Tauri 应用",
    });
  }
  try {
    return (await invoke(command, args)) as T;
  } catch (raw) {
    throw new TtsCommandError((raw ?? {}) as never);
  }
}

export function isPartialFailure(error: unknown): boolean {
  return error instanceof TtsCommandError && error.code === "E_PARTIAL";
}

export function audioUrl(path: string): string {
  return convertFileSrc(path);
}

export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "0.0s";
  return `${(ms / 1000).toFixed(1)}s`;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

export const api = {
  appInfo: () => call<AppInfo>("app_info"),
  configGet: (reveal = false) => call<ConfigPayload>("config_get", { reveal }),
  configSet: (key: string, value: string) =>
    call<{ config_path: string; config: WorkspaceConfig }>("config_set", { key, value }),
  doctor: (skipNetwork = false) =>
    call<DoctorReport>("doctor_run", { skipNetwork }),
  projectList: () => call<ProjectList>("project_list"),
  projectInit: (path: string, name: string) =>
    call<{ path: string; name: string }>("project_init", { path, name }),
  projectImport: (
    project: string,
    script: string,
    narratorFallback = true,
    joinWithSpace = false,
  ) =>
    call<ImportOutcome>("project_import", {
      project,
      script,
      narratorFallback,
      joinWithSpace,
    }),
  projectDetail: (project: string) =>
    call<ProjectDetail>("project_detail", { project }),
  roleBind: (project: string, role: string, voice: string | null, style: string | null) =>
    call<{ role: string; voice: string | null; style: string | null }>("role_bind", {
      project,
      role,
      voice,
      style,
    }),
  lineSet: (
    project: string,
    lineId: string,
    voice: string | null,
    style: string | null,
    clearVoice = false,
    clearStyle = false,
  ) =>
    call<{ line_id: string }>("line_set", {
      project,
      lineId,
      voice,
      style,
      clearVoice,
      clearStyle,
    }),
  takesSelect: (project: string, lineId: string, takeId: string) =>
    call<{ line_id: string; take_id: string }>("takes_select", {
      project,
      lineId,
      takeId,
    }),
  synthLine: (
    project: string,
    lineId: string,
    voice: string | null = null,
    style: string | null = null,
    noCache = false,
  ) =>
    call<{
      line_id: string;
      take_id: string;
      voice: string;
      model: string;
      duration_ms: number;
      latency_ms: number;
      cached: boolean;
    }>("synth_line", { project, lineId, voice, style, noCache }),
  batchRun: (
    project: string,
    options: { concurrency?: number | null; scene?: string | null; lineIds?: string[]; force?: boolean },
    onEvent: Channel<BatchEvent>,
  ) =>
    call<BatchResult>("batch_run", {
      project,
      concurrency: options.concurrency ?? null,
      scene: options.scene ?? null,
      lineIds: options.lineIds ?? [],
      force: options.force ?? false,
      onEvent,
    }),
  exportRun: (
    project: string,
    options: {
      format: string;
      layout: string;
      out?: string | null;
      nameTemplate?: string | null;
      concat?: boolean;
      normalize?: boolean;
      trimSilence?: boolean;
      overwrite?: boolean;
    },
  ) =>
    call<ExportResult>("export_run", {
      project,
      format: options.format,
      layout: options.layout,
      out: options.out ?? null,
      nameTemplate: options.nameTemplate ?? null,
      concat: options.concat ?? false,
      normalize: options.normalize ?? false,
      trimSilence: options.trimSilence ?? false,
      overwrite: options.overwrite ?? false,
    }),
  usageGet: (since?: string | null) =>
    call<UsageResult>("usage_get", { since: since ?? null }),
  gcRun: (dryRun = true) => call<GcResult>("gc_run", { dryRun }),
  voiceList: (kind?: string | null) =>
    call<VoiceList>("voice_list", { kind: kind ?? null }),
  voiceRemove: (name: string) =>
    call<{ id: string; name: string; kind: string }>("voice_remove", { name }),
  designCreate: (prompt: string, text: string, name?: string | null, optimize = false) =>
    call<{
      output: string;
      duration_ms: number;
      latency_ms: number;
      cached: boolean;
      voice: { id: string; name: string } | null;
    }>("design_create", { prompt, text, name: name ?? null, optimize }),
  designSolidify: (from: string, name: string) =>
    call<{ id: string; name: string; kind: string; source_voice: string | null }>(
      "design_solidify",
      { from, name },
    ),
  cloneCreate: (sample: string, name: string) =>
    call<{ id: string; name: string }>("clone_create", { sample, name }),
  revealPath: (path: string) => call<{ path: string }>("reveal_path", { path }),
};

export { Channel };
