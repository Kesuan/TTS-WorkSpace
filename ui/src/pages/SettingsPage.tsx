import { useCallback, useEffect, useState } from "react";

import { Button, Card, Input, Tag, notificationOpen } from "animal-island-ui";

import { api } from "../api";
import type { ConfigPayload, DoctorReport } from "../types";

function notifyError(error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  notificationOpen({ message: "操作失败", description: message, type: "error" });
}

function notifySuccess(description: string) {
  notificationOpen({ message: "已保存", description, type: "success" });
}

const STATUS_COLOR: Record<string, "app-green" | "app-yellow" | "app-red" | "default"> = {
  ok: "app-green",
  warn: "app-yellow",
  fail: "app-red",
  skipped: "default",
};

export default function SettingsPage() {
  const [payload, setPayload] = useState<ConfigPayload | null>(null);
  const [report, setReport] = useState<DoctorReport | null>(null);
  const [busy, setBusy] = useState(false);

  const [apiKey, setApiKey] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [modelTts, setModelTts] = useState("");
  const [modelDesign, setModelDesign] = useState("");
  const [modelClone, setModelClone] = useState("");
  const [concurrency, setConcurrency] = useState("");
  const [retryMax, setRetryMax] = useState("");
  const [retryBackoff, setRetryBackoff] = useState("");
  const [ffmpegPath, setFfmpegPath] = useState("");

  const refresh = useCallback(async () => {
    try {
      const next = await api.configGet(false);
      setPayload(next);
      setBaseUrl(next.config.api.base_url);
      setModelTts(next.config.models.tts);
      setModelDesign(next.config.models.design);
      setModelClone(next.config.models.clone);
      setConcurrency(String(next.config.generation.concurrency));
      setRetryMax(String(next.config.generation.retry.max));
      setRetryBackoff(String(next.config.generation.retry.backoff_ms));
      setFfmpegPath(next.config.ffmpeg.path);
    } catch (error) {
      notifyError(error);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const save = async (key: string, value: string, label: string) => {
    setBusy(true);
    try {
      await api.configSet(key, value);
      notifySuccess(label);
      await refresh();
    } catch (error) {
      notifyError(error);
    } finally {
      setBusy(false);
    }
  };

  const runDoctor = async () => {
    setBusy(true);
    try {
      setReport(await api.doctor(false));
    } catch (error) {
      notifyError(error);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="grid-two">
      <div className="stack">
        <Card color="app-teal" pattern="default">
          <div className="stack">
            <strong>API 配置</strong>
            {payload ? (
              <span className="tiny muted">
                {`Key 来源：${payload.api_key_source} · 配置：${payload.config_path}`}
              </span>
            ) : null}
            <Input
              size="small"
              type="password"
              placeholder={
                payload?.config.api.api_key
                  ? `当前：${payload.config.api.api_key}（输入新 Key 以替换）`
                  : "API Key（sk-…）"
              }
              value={apiKey}
              onChange={(event) => setApiKey(event.target.value)}
            />
            <div className="row">
              <Button
                size="small"
                type="primary"
                loading={busy}
                onClick={() => {
                  if (!apiKey.trim()) {
                    notificationOpen({ message: "提示", description: "请输入 API Key", type: "warning" });
                    return;
                  }
                  void save("api.api_key", apiKey.trim(), "API Key 已更新").then(() => setApiKey(""));
                }}
              >
                保存 Key
              </Button>
              <Button size="small" onClick={() => void save("api.api_key", "", "API Key 已清除")}>
                清除 Key
              </Button>
            </div>
            <Input
              size="small"
              value={baseUrl}
              onChange={(event) => setBaseUrl(event.target.value)}
              placeholder="Base URL"
            />
            <div className="row">
              <Button
                size="small"
                loading={busy}
                onClick={() => void save("api.base_url", baseUrl.trim(), "Base URL 已更新")}
              >
                保存 Base URL
              </Button>
            </div>
          </div>
        </Card>

        <Card color="app-blue" pattern="none">
          <div className="stack">
            <strong>模型 ID</strong>
            <span className="tiny muted">平台模型下线/更名时在此切换，无需改代码</span>
            <Input size="small" value={modelTts} onChange={(event) => setModelTts(event.target.value)} />
            <div className="row">
              <Button size="small" loading={busy} onClick={() => void save("models.tts", modelTts.trim(), "models.tts 已更新")}>
                保存预置模型
              </Button>
            </div>
            <Input size="small" value={modelDesign} onChange={(event) => setModelDesign(event.target.value)} />
            <div className="row">
              <Button size="small" loading={busy} onClick={() => void save("models.design", modelDesign.trim(), "models.design 已更新")}>
                保存设计模型
              </Button>
            </div>
            <Input size="small" value={modelClone} onChange={(event) => setModelClone(event.target.value)} />
            <div className="row">
              <Button size="small" loading={busy} onClick={() => void save("models.clone", modelClone.trim(), "models.clone 已更新")}>
                保存克隆模型
              </Button>
            </div>
          </div>
        </Card>

        <Card color="app-yellow" pattern="none">
          <div className="stack">
            <strong>生成与工具</strong>
            <div className="row">
              <Input
                size="small"
                style={{ width: 100 }}
                value={concurrency}
                onChange={(event) => setConcurrency(event.target.value)}
              />
              <Button size="small" loading={busy} onClick={() => void save("generation.concurrency", concurrency.trim(), "并发数已更新")}>
                保存并发数
              </Button>
              <Input
                size="small"
                style={{ width: 100 }}
                value={retryMax}
                onChange={(event) => setRetryMax(event.target.value)}
              />
              <Button size="small" loading={busy} onClick={() => void save("generation.retry.max", retryMax.trim(), "重试次数已更新")}>
                保存重试次数
              </Button>
            </div>
            <div className="row">
              <Input
                size="small"
                style={{ width: 120 }}
                value={retryBackoff}
                onChange={(event) => setRetryBackoff(event.target.value)}
              />
              <Button size="small" loading={busy} onClick={() => void save("generation.retry.backoff_ms", retryBackoff.trim(), "退避基数已更新")}>
                保存退避基数(ms)
              </Button>
            </div>
            <Input size="small" value={ffmpegPath} onChange={(event) => setFfmpegPath(event.target.value)} />
            <div className="row">
              <Button size="small" loading={busy} onClick={() => void save("ffmpeg.path", ffmpegPath.trim(), "ffmpeg 路径已更新")}>
                保存 ffmpeg 路径
              </Button>
            </div>
          </div>
        </Card>
      </div>

      <Card color="default" pattern="none">
        <div className="stack">
          <div className="row-between">
            <strong>环境自检</strong>
            <Button size="small" loading={busy} onClick={() => void runDoctor()}>
              运行 doctor
            </Button>
          </div>
          {payload ? <span className="tiny mono muted">{payload.workspace_root}</span> : null}
          {report ? (
            <div className="stack">
              {report.checks.map((check) => (
                <div className="row" key={check.name}>
                  <Tag size="small" variant="soft" color={STATUS_COLOR[check.status] ?? "default"}>
                    {check.status}
                  </Tag>
                  <strong className="tiny" style={{ minWidth: 80 }}>
                    {check.name}
                  </strong>
                  <span className="tiny muted">{check.detail}</span>
                </div>
              ))}
              <span className="tiny">
                {`ok ${report.summary.ok} / warn ${report.summary.warn} / fail ${report.summary.fail} / skip ${report.summary.skipped}`}
              </span>
              {report.checks
                .filter((check) => check.hint)
                .map((check) => (
                  <span className="tiny mono muted" key={`${check.name}-hint`}>
                    {`${check.name} → ${check.hint}`}
                  </span>
                ))}
            </div>
          ) : (
            <span className="muted tiny">点击「运行 doctor」检查 Key / ffmpeg / 网络连通性</span>
          )}
        </div>
      </Card>
    </div>
  );
}
