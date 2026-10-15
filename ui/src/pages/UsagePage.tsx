import { useCallback, useEffect, useState } from "react";
import type { ReactNode } from "react";

import { Button, Card, Input, Tag, notificationOpen } from "animal-island-ui";
import {
  BadgeCheckIcon,
  ClockIcon,
  EyeIcon,
  FileIcon,
  InboxIcon,
  MusicIcon,
  RocketIcon,
  SendIcon,
  TrashIcon,
} from "naive-icons";

import { api, formatBytes, formatDuration } from "../api";
import type { GcResult, UsageResult } from "../types";

function notifyError(error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  notificationOpen({ message: "操作失败", description: message, type: "error" });
}

function Stat({
  label,
  value,
  icon,
  tone,
}: {
  label: string;
  value: string;
  icon: ReactNode;
  tone: "teal" | "blue" | "green" | "yellow" | "orange" | "pink";
}) {
  return (
    <Card color="default" pattern="none">
      <div className="stat-card">
        <span className={`stat-icon stat-icon-${tone}`}>{icon}</span>
        <div className="stat-card-text">
          <span className="tiny muted">{label}</span>
          <span className="stat-value">{value}</span>
        </div>
      </div>
    </Card>
  );
}

export default function UsagePage() {
  const [usage, setUsage] = useState<UsageResult | null>(null);
  const [since, setSince] = useState("");
  const [gc, setGc] = useState<GcResult | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setUsage(await api.usageGet(since.trim() ? since.trim() : null));
    } catch (error) {
      notifyError(error);
    }
  }, [since]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const runGc = async (dryRun: boolean) => {
    setBusy(true);
    try {
      const result = await api.gcRun(dryRun);
      setGc(result);
      notificationOpen({
        message: dryRun ? "缓存预览" : "缓存清理完成",
        description: `${dryRun ? "将删除" : "已删除"} ${result.removed} 个未引用缓存（${formatBytes(
          result.freed_bytes,
        )}），保留 ${result.kept} 个`,
        type: "info",
      });
    } catch (error) {
      notifyError(error);
    } finally {
      setBusy(false);
    }
  };

  const summary = usage?.summary;

  return (
    <div className="stack">
      <Card color="app-blue" pattern="default">
        <div className="row-between">
          <span className="section-title">
            <InboxIcon size={16} />
            用量统计（来自审计日志）
          </span>
          <div className="row">
            <Input
              size="small"
              style={{ width: 160 }}
              placeholder="--since（如 2026-10）"
              value={since}
              onChange={(event) => setSince(event.target.value)}
            />
            <Button size="small" onClick={() => void refresh()}>
              刷新
            </Button>
          </div>
        </div>
        {usage ? <span className="tiny mono muted">{usage.audit_file}</span> : null}
      </Card>

      {summary ? (
        <>
          <div className="stat-grid">
            <Stat label="请求总数" value={String(summary.requests)} tone="teal" icon={<SendIcon size={18} />} />
            <Stat
              label="成功 / 失败"
              value={`${summary.ok} / ${summary.failed}`}
              tone="green"
              icon={<BadgeCheckIcon size={18} />}
            />
            <Stat
              label="缓存命中"
              value={`${summary.cached}（${(summary.cache_hit_rate * 100).toFixed(1)}%）`}
              tone="blue"
              icon={<InboxIcon size={18} />}
            />
            <Stat label="字符数" value={String(summary.chars)} tone="yellow" icon={<FileIcon size={18} />} />
            <Stat
              label="音频总时长"
              value={formatDuration(summary.audio_duration_ms)}
              tone="pink"
              icon={<MusicIcon size={18} />}
            />
            <Stat
              label="平均延迟"
              value={`${Math.round(summary.latency_ms_total / Math.max(1, summary.requests))} ms`}
              tone="orange"
              icon={<RocketIcon size={18} />}
            />
          </div>

          <div className="grid-two">
            <Card color="default" pattern="none">
              <div className="stack">
                <span className="section-title">
                  <SendIcon size={16} />
                  按命令
                </span>
                {Object.keys(summary.by_cmd).length === 0 ? (
                  <span className="tiny muted">暂无记录</span>
                ) : (
                  <table className="data-table">
                    <tbody>
                      {Object.entries(summary.by_cmd).map(([name, count]) => (
                        <tr key={name}>
                          <td className="mono">{name}</td>
                          <td style={{ width: 80 }}>{count}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </div>
            </Card>
            <Card color="default" pattern="none">
              <div className="stack">
                <span className="section-title">
                  <MusicIcon size={16} />
                  按模型
                </span>
                {Object.keys(summary.by_model).length === 0 ? (
                  <span className="tiny muted">暂无记录</span>
                ) : (
                  <table className="data-table">
                    <tbody>
                      {Object.entries(summary.by_model).map(([name, count]) => (
                        <tr key={name}>
                          <td className="mono">{name}</td>
                          <td style={{ width: 80 }}>{count}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </div>
            </Card>
          </div>

          <Card color="app-yellow" pattern="none">
            <div className="row-between">
              <div className="row">
                <Tag size="small" variant="soft">
                  {summary.cost_note}
                </Tag>
                {summary.first_ts ? (
                  <span className="tiny muted">{`${summary.first_ts} ~ ${summary.last_ts ?? "…"}`}</span>
                ) : null}
              </div>
            </div>
          </Card>
        </>
      ) : (
        <Card color="default">
          <div className="empty-state">
            <ClockIcon size={32} color="#c4b89e" />
            <span className="empty-state-title">暂无用量数据</span>
            <span className="tiny">生成一次音频后即可看到统计</span>
          </div>
        </Card>
      )}

      <Card color="app-orange" pattern="none">
        <div className="stack">
          <span className="section-title">
            <TrashIcon size={16} />
            缓存清理
          </span>
          <span className="hint-solid">删除未被任何项目 take 引用的缓存音频（`tts gc`）</span>
          <div className="row">
            <Button
              size="small"
              icon={<EyeIcon size={14} />}
              loading={busy}
              onClick={() => void runGc(true)}
            >
              预览（dry-run）
            </Button>
            <Button
              size="small"
              danger
              icon={<TrashIcon size={14} />}
              loading={busy}
              onClick={() => void runGc(false)}
            >
              执行清理
            </Button>
          </div>
          {gc ? (
            <div className="tiny mono">
              {`缓存目录：${gc.cache_dir}`}
              <br />
              {`扫描项目 ${gc.scanned_projects} 个 · 引用键 ${gc.referenced_keys} 个 · ${
                gc.dry_run ? "将删除" : "已删除"
              } ${gc.removed} 个（${formatBytes(gc.freed_bytes)}）· 保留 ${gc.kept} 个`}
            </div>
          ) : null}
        </div>
      </Card>
    </div>
  );
}
