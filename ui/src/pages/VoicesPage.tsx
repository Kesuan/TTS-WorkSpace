import { useCallback, useEffect, useState } from "react";

import { Button, Card, Input, Modal, Select, Switch, Tag, notificationOpen } from "animal-island-ui";

import { api } from "../api";
import type { VoiceInfo } from "../types";

function notifyError(error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  notificationOpen({ message: "操作失败", description: message, type: "error" });
}

function notifySuccess(description: string) {
  notificationOpen({ message: "完成", description, type: "success" });
}

function KindTag({ voice }: { voice: VoiceInfo }) {
  const color: "app-teal" | "purple" | "app-orange" | "default" = voice.builtin
    ? "app-teal"
    : voice.kind === "design"
      ? "purple"
      : voice.kind === "clone"
        ? "app-orange"
        : "default";
  const label = voice.builtin ? "预置" : voice.kind === "design" ? "设计" : voice.kind === "clone" ? "克隆" : voice.kind;
  return (
    <Tag size="small" variant="soft" color={color}>
      {label}
    </Tag>
  );
}

export default function VoicesPage() {
  const [voices, setVoices] = useState<VoiceInfo[]>([]);
  const [kind, setKind] = useState("all");
  const [busy, setBusy] = useState(false);

  const [designPrompt, setDesignPrompt] = useState("");
  const [designText, setDesignText] = useState("");
  const [designName, setDesignName] = useState("");
  const [optimize, setOptimize] = useState(true);

  const [cloneSample, setCloneSample] = useState("");
  const [cloneName, setCloneName] = useState("");

  const [solidifyFrom, setSolidifyFrom] = useState("");
  const [solidifyName, setSolidifyName] = useState("");

  const [removing, setRemoving] = useState<VoiceInfo | null>(null);

  const refresh = useCallback(async () => {
    try {
      const payload = await api.voiceList(kind === "all" ? null : kind);
      setVoices(payload.voices);
    } catch (error) {
      notifyError(error);
    }
  }, [kind]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const runDesign = async () => {
    if (!designPrompt.trim() || !designText.trim()) {
      notificationOpen({ message: "提示", description: "音色描述与试听文本都不能为空", type: "warning" });
      return;
    }
    setBusy(true);
    try {
      const result = await api.designCreate(
        designPrompt,
        designText,
        designName.trim() ? designName.trim() : null,
        optimize,
      );
      notifySuccess(
        `设计生成完成（${(result.duration_ms / 1000).toFixed(1)}s${result.cached ? "，缓存" : ""}）${
          result.voice ? `，已入库「${result.voice.name}」` : "；填写名称可入库"
        }`,
      );
      setDesignPrompt("");
      setDesignText("");
      setDesignName("");
      await refresh();
    } catch (error) {
      notifyError(error);
    } finally {
      setBusy(false);
    }
  };

  const runClone = async () => {
    if (!cloneSample.trim() || !cloneName.trim()) {
      notificationOpen({ message: "提示", description: "样本路径与音色名都不能为空", type: "warning" });
      return;
    }
    setBusy(true);
    try {
      const created = await api.cloneCreate(cloneSample.trim(), cloneName.trim());
      notifySuccess(`已创建克隆音色「${created.name}」`);
      setCloneSample("");
      setCloneName("");
      await refresh();
    } catch (error) {
      notifyError(error);
    } finally {
      setBusy(false);
    }
  };

  const runSolidify = async () => {
    if (!solidifyFrom.trim() || !solidifyName.trim()) {
      notificationOpen({ message: "提示", description: "来源与名称都不能为空", type: "warning" });
      return;
    }
    setBusy(true);
    try {
      const created = await api.designSolidify(solidifyFrom.trim(), solidifyName.trim());
      notifySuccess(`已固化为克隆音色「${created.name}」（跨句一致）`);
      setSolidifyFrom("");
      setSolidifyName("");
      await refresh();
    } catch (error) {
      notifyError(error);
    } finally {
      setBusy(false);
    }
  };

  const confirmRemove = async () => {
    if (!removing) return;
    setBusy(true);
    try {
      await api.voiceRemove(removing.name);
      notifySuccess(`已删除音色「${removing.name}」`);
      setRemoving(null);
      await refresh();
    } catch (error) {
      notifyError(error);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="stack">
      <Card color="app-pink" pattern="default">
        <div className="stack">
          <div className="row-between">
            <strong>音色设计（voicedesign）</strong>
            <span className="tiny muted">描述越具体越好；建议随后「固化」以保证跨句一致</span>
          </div>
          <textarea
            className="mono"
            style={{ minHeight: 64, width: "100%", padding: 8, borderRadius: 8 }}
            placeholder="音色描述：如「二十岁出头的年轻女声，音色清亮偏薄，带一点书卷气，语速中等偏慢」"
            value={designPrompt}
            onChange={(event) => setDesignPrompt(event.target.value)}
          />
          <textarea
            style={{ minHeight: 48, width: "100%", padding: 8, borderRadius: 8 }}
            placeholder="试听文本（3~10 秒为宜）"
            value={designText}
            onChange={(event) => setDesignText(event.target.value)}
          />
          <div className="row">
            <Input
              size="small"
              style={{ width: 200 }}
              placeholder="音色名（填写即入库）"
              value={designName}
              onChange={(event) => setDesignName(event.target.value)}
            />
            <span className="tiny">智能润色文本</span>
            <Switch size="small" checked={optimize} onChange={setOptimize} />
            <Button size="small" type="primary" loading={busy} onClick={() => void runDesign()}>
              生成试听
            </Button>
          </div>
        </div>
      </Card>

      <div className="grid-two">
        <div className="stack">
          <Card color="app-orange" pattern="none">
            <div className="stack">
              <strong>克隆音色</strong>
              <span className="tiny muted">样本要求：wav / mp3，base64 后 ≤10MB</span>
              <Input
                size="small"
                placeholder="样本文件路径"
                value={cloneSample}
                onChange={(event) => setCloneSample(event.target.value)}
              />
              <Input
                size="small"
                placeholder="音色名"
                value={cloneName}
                onChange={(event) => setCloneName(event.target.value)}
              />
              <Button size="small" loading={busy} onClick={() => void runClone()}>
                创建克隆音色
              </Button>
            </div>
          </Card>

          <Card color="app-green" pattern="none">
            <div className="stack">
              <strong>设计 → 固化</strong>
              <span className="tiny muted">把设计音色的留档样本固化为克隆音色</span>
              <Input
                size="small"
                placeholder="来源：design 音色名，或音频文件路径"
                value={solidifyFrom}
                onChange={(event) => setSolidifyFrom(event.target.value)}
              />
              <Input
                size="small"
                placeholder="新音色名"
                value={solidifyName}
                onChange={(event) => setSolidifyName(event.target.value)}
              />
              <Button size="small" loading={busy} onClick={() => void runSolidify()}>
                固化
              </Button>
            </div>
          </Card>
        </div>

        <Card color="default" pattern="none">
          <div className="stack">
            <div className="row-between">
              <strong>音色列表</strong>
              <div className="row">
                <Select
                  options={[
                    { key: "all", label: "全部" },
                    { key: "preset", label: "预置" },
                    { key: "design", label: "设计" },
                    { key: "clone", label: "克隆" },
                  ]}
                  value={kind}
                  onChange={setKind}
                />
                <Button size="small" onClick={() => void refresh()}>
                  刷新
                </Button>
              </div>
            </div>
            <table className="data-table">
              <thead>
                <tr>
                  <th style={{ width: 60 }}>类型</th>
                  <th style={{ width: 160 }}>名称</th>
                  <th>ID / 说明</th>
                  <th style={{ width: 90 }}>样本</th>
                  <th style={{ width: 70 }} />
                </tr>
              </thead>
              <tbody>
                {voices.map((voice) => (
                  <tr key={`${voice.kind}-${voice.id}`}>
                    <td>
                      <KindTag voice={voice} />
                    </td>
                    <td>{voice.name}</td>
                    <td className="mono tiny">
                      {voice.builtin
                        ? `${voice.language ?? ""} ${voice.gender ?? ""}`
                        : voice.kind === "design"
                          ? (voice.design_prompt ?? voice.id)
                          : voice.source_voice
                            ? `${voice.id}（固化自 ${voice.source_voice}）`
                            : voice.id}
                    </td>
                    <td className="tiny">{voice.has_sample ? "有" : "-"}</td>
                    <td>
                      {voice.builtin ? null : (
                        <Button size="small" type="text" danger onClick={() => setRemoving(voice)}>
                          删除
                        </Button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </Card>
      </div>

      <Modal
        open={removing !== null}
        title="删除音色"
        onClose={() => setRemoving(null)}
        footer={
          <div className="row">
            <Button size="small" onClick={() => setRemoving(null)}>
              取消
            </Button>
            <Button size="small" danger loading={busy} onClick={() => void confirmRemove()}>
              确认删除
            </Button>
          </div>
        }
      >
        <span>
          {`将删除音色「${removing?.name ?? ""}」及其样本文件，此操作不可撤销。`}
        </span>
      </Modal>
    </div>
  );
}
