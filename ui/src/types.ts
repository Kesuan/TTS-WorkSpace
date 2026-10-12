// 与 Rust 侧（crates/desktop + tts-core）对齐的类型定义。

export interface Counts {
  total: number;
  pending: number;
  ok: number;
  failed: number;
  removed: number;
}

export interface ProjectSummary {
  path: string;
  name: string;
  scripts: string[];
  counts: Counts;
}

export interface ProjectList {
  projects_dir: string;
  projects: ProjectSummary[];
}

export interface RoleInfo {
  name: string;
  bound: boolean;
  voice: string | null;
  style: string | null;
}

export interface TakeInfo {
  take_id: string;
  file: string;
  abs_file: string;
  duration_ms: number;
  chars: number;
  created_at: string;
  selected: boolean;
}

export interface LineInfo {
  id: string;
  chapter: string | null;
  scene: string | null;
  index: number;
  role: string;
  text: string;
  status: string;
  selected_take: string | null;
  overrides: { voice: string | null; style: string | null };
  effective_voice: string | null;
  effective_style: string | null;
  takes: TakeInfo[];
}

export interface ProjectDetail {
  path: string;
  name: string;
  scripts: string[];
  counts: Counts;
  roles: RoleInfo[];
  lines: LineInfo[];
}

export interface VoiceInfo {
  id: string;
  name: string;
  kind: "preset" | "design" | "clone";
  builtin: boolean;
  model: string | null;
  preset_name: string | null;
  design_prompt: string | null;
  source_voice: string | null;
  sample_file: string | null;
  sample_sha256: string | null;
  tags: string[];
  notes: string;
  created_at: string | null;
  has_sample: boolean;
  language: string | null;
  gender: string | null;
}

export interface VoiceList {
  library_dir: string;
  total: number;
  voices: VoiceInfo[];
}

export interface BatchSummary {
  total: number;
  succeeded: number;
  failed: number;
  skipped: number;
  cached: number;
  concurrency: number;
  total_duration_ms: number;
}

export interface FailedLine {
  line_id: string;
  role: string;
  code: string;
  message: string;
}

export interface BatchResult {
  summary: BatchSummary;
  failed_lines: FailedLine[];
  partial?: boolean;
}

export type BatchEvent =
  | { event: "started"; total: number; concurrency: number; skipped: number }
  | { event: "line_started"; line_id: string; role: string }
  | { event: "line_skipped"; line_id: string; role: string; reason: string }
  | {
      event: "line_succeeded";
      line_id: string;
      role: string;
      take_id: string;
      duration_ms: number;
      latency_ms: number;
      cached: boolean;
    }
  | {
      event: "line_failed";
      line_id: string;
      role: string;
      code: string;
      message: string;
      attempts: number;
    }
  | {
      event: "finished";
      succeeded: number;
      failed: number;
      skipped: number;
      cached: number;
      total_duration_ms: number;
    };

export interface ExportItem {
  line_id: string;
  role: string;
  scene: string | null;
  chapter: string | null;
  index: number;
  take_id: string;
  file: string;
  duration_ms: number;
  chars: number;
}

export interface ExportResult {
  out_dir: string;
  format: string;
  layout: string;
  manifest: string;
  concat: string | null;
  ffmpeg_used: boolean;
  count: number;
  items: ExportItem[];
}

export interface UsageSummary {
  requests: number;
  ok: number;
  failed: number;
  cached: number;
  chars: number;
  audio_duration_ms: number;
  latency_ms_total: number;
  cache_hit_rate: number;
  by_cmd: Record<string, number>;
  by_model: Record<string, number>;
  first_ts?: string;
  last_ts?: string;
  cost_note: string;
}

export interface UsageResult {
  audit_file: string;
  since: string | null;
  summary: UsageSummary;
}

export interface GcResult {
  cache_dir: string;
  scanned_projects: number;
  referenced_keys: number;
  removed: number;
  freed_bytes: number;
  kept: number;
  dry_run: boolean;
}

export interface DoctorCheck {
  name: string;
  status: "ok" | "warn" | "fail" | "skipped";
  detail: string;
  hint?: string;
}

export interface DoctorReport {
  version: string;
  workspace_root: string;
  config_path: string;
  checks: DoctorCheck[];
  summary: { ok: number; warn: number; fail: number; skipped: number };
}

export interface AppInfo {
  version: string;
  workspace_root: string;
  config_path: string;
}

export interface WorkspaceConfig {
  schema_version: number;
  api: { base_url: string; api_key: string | null; timeout_ms: number };
  models: { tts: string; design: string; clone: string };
  generation: {
    concurrency: number;
    max_chars_per_request: number;
    retry: { max: number; backoff_ms: number };
  };
  ffmpeg: { path: string };
  workspace_root?: string | null;
}

export interface ConfigPayload {
  config_path: string;
  workspace_root: string;
  api_key_source: "env" | "config" | "missing";
  config: WorkspaceConfig;
}

export interface ImportOutcome {
  script: string;
  counts: {
    lines: number;
    roles: number;
    chapters: number;
    scenes: number;
    narrator_lines: number;
  };
  roles: string[];
  unbound_roles: string[];
  warnings: { line: number; code: string; message: string }[];
  diff: { added: number; removed: number; changed: number; unchanged: number };
}
