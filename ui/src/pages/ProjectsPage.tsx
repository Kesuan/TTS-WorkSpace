import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  Button,
  Card,
  Input,
  Modal,
  Progress,
  Select,
  Switch,
  Tag,
  notificationOpen,
} from "animal-island-ui";

import {
  Channel,
  api,
  audioUrl,
  formatDuration,
  isPartialFailure,
  TtsCommandError,
} from "../api";
import type {
  BatchEvent,
  ExportResult,
  LineInfo,
  ProjectDetail,
  ProjectSummary,
} from "../types";

const STATUS_TAG: Record<string, { label: string; color: "default" | "app-green" | "app-red" | "brown" }> = {
  pending: { label: "待生成", color: "default" },
  ok: { label: "已生成", color: "app-green" },
  failed: { label: "失败", color: "app-red" },
  removed: { label: "已移除", color: "brown" },
};

function notifyError(error: unknown) {
  const message =
    error instanceof TtsCommandError
      ? error.message
      : error instanceof Error
        ? error.message
        : String(error);
  notificationOpen({ message: "操作失败", description: message, type: "error" });
}

function notifySuccess(description: string) {
  notificationOpen({ message: "完成", description, type: "success" });
}

function summarizeEvent(event: BatchEvent): string {
  switch (event.event) {
    case "started":
      return `开始：${event.total} 条（并发 ${event.concurrency}，跳过 ${event.skipped}）`;
    case "line_started":
      return `· ${event.line_id} [${event.role}]`;
    case "line_skipped":
      return `· ${event.line_id} 跳过（${event.reason}）`;
    case "line_succeeded":
      return `✓ ${event.line_id} → take ${event.take_id}（${formatDuration(event.duration_ms)}${
        event.cached ? "，缓存" : ""
      }）`;
    case "line_failed":
      return `✗ ${event.line_id} 失败（${event.code}）：${event.message}`;
    case "finished":
      return `完成：成功 ${event.succeeded} / 失败 ${event.failed} / 跳过 ${event.skipped}（缓存 ${event.cached}）`;
    default:
      return "";
  }
}

function TakeRow({
  line,
  projectPath,
  onChanged,
}: {
  line: LineInfo;
  projectPath: string;
  onChanged: () => void;
}) {
  const selectTake = async (takeId: string) => {
    try {
      await api.takesSelect(projectPath, line.id, takeId);
      onChanged();
    } catch (error) {
      notifyError(error);
    }
  };

  if (line.takes.length === 0) {
    return <span className="muted tiny">暂无 take</span>;
  }

  return (
    <div className="stack">
      {line.takes.map((take) => (
        <div className="take-row" key={take.take_id}>
          <Tag size="small" variant={take.selected ? "solid" : "outlined"} color={take.selected ? "app-teal" : "default"}>
            {take.take_id}
          </Tag>
          <span className="tiny muted">{formatDuration(take.duration_ms)}</span>
          <audio controls preload="none" src={audioUrl(take.abs_file)} style={{ height: 32 }} />
          {take.selected ? (
            <Tag size="small" variant="soft" color="app-green">
              已选用
            </Tag>
          ) : (
            <Button size="small" type="text" onClick={() => void selectTake(take.take_id)}>
              选用
            </Button>
          )}
        </div>
      ))}
    </div>
  );
}

function LineCard({
  line,
  projectPath,
  onChanged,
}: {
  line: LineInfo;
  projectPath: string;
  onChanged: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [voice, setVoice] = useState(line.overrides.voice ?? "");
  const [style, setStyle] = useState(line.overrides.style ?? "");
  const [busy, setBusy] = useState(false);

  const status = STATUS_TAG[line.status] ?? STATUS_TAG.pending;

  const regenerate = async () => {
    setBusy(true);
    try {
      const result = await api.synthLine(projectPath, line.id);
      notifySuccess(
        `${line.id} → take ${result.take_id}（${result.model}，${formatDuration(result.duration_ms)}${
          result.cached ? "，缓存命中" : ""
        }）`,
      );
      onChanged();
    } catch (error) {
      notifyError(error);
    } finally {
      setBusy(false);
    }
  };

  const saveOverride = async (clearVoice: boolean, clearStyle: boolean) => {
    setBusy(true);
    try {
      await api.lineSet(
        projectPath,
        line.id,
        voice.trim() ? voice.trim() : null,
        style.trim() ? style.trim() : null,
        clearVoice,
        clearStyle,
      );
      notifySuccess("行级覆盖已更新");
      setEditing(false);
      onChanged();
    } catch (error) {
      notifyError(error);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card color="default" pattern="none">
      <div className="line-card">
        <div className="line-head">
          <Tag size="small" variant="outlined">
            {`#${line.index}`}
          </Tag>
          <Tag size="small" variant="soft" color="app-blue">
            {line.role}
          </Tag>
          <Tag size="small" variant="soft" color={status.color}>
            {status.label}
          </Tag>
          {line.scene ? (
            <Tag size="small" variant="outlined" color="app-yellow">
              {line.scene}
            </Tag>
          ) : null}
          <span className="mono muted">{line.id}</span>
          <div className="grow" />
          <Button size="small" onClick={() => setEditing((value) => !value)}>
            覆盖
          </Button>
          <Button size="small" type="primary" loading={busy} onClick={() => void regenerate()}>
            重新生成
          </Button>
        </div>
        <div className="line-text">{line.text}</div>
        <div className="row tiny muted">
          <span>音色：{line.effective_voice ?? "未绑定"}</span>
          {line.effective_style ? <span>风格：{line.effective_style}</span> : null}
        </div>
        {editing ? (
          <div className="stack">
            <div className="row">
              <Input
                size="small"
                value={voice}
                placeholder="行级音色（预置名 / 库内音色 / design:… / clone:…）"
                onChange={(event) => setVoice(event.target.value)}
              />
              <Input
                size="small"
                value={style}
                placeholder="行级风格指令（自然语言）"
                onChange={(event) => setStyle(event.target.value)}
              />
            </div>
            <div className="row">
              <Button size="small" type="primary" loading={busy} onClick={() => void saveOverride(false, false)}>
                保存覆盖
              </Button>
              <Button size="small" onClick={() => void saveOverride(true, true)}>
                清除覆盖
              </Button>
            </div>
          </div>
        ) : null}
        <TakeRow line={line} projectPath={projectPath} onChanged={onChanged} />
      </div>
    </Card>
  );
}

function RoleBindingTable({
  detail,
  onChanged,
}: {
  detail: ProjectDetail;
  onChanged: () => void;
}) {
  const [drafts, setDrafts] = useState<Record<string, { voice: string; style: string }>>({});
  const [busy, setBusy] = useState<string | null>(null);

  const draftOf = (role: string) =>
    drafts[role] ?? { voice: "", style: "" };

  const bind = async (role: string) => {
    const draft = draftOf(role);
    if (!draft.voice.trim() && !draft.style.trim()) {
      notificationOpen({ message: "提示", description: "请填写音色或风格指令", type: "warning" });
      return;
    }
    setBusy(role);
    try {
      await api.roleBind(
        detail.path,
        role,
        draft.voice.trim() ? draft.voice.trim() : null,
        draft.style.trim() ? draft.style.trim() : null,
      );
      notifySuccess(`角色「${role}」已绑定`);
      setDrafts((prev) => ({ ...prev, [role]: { voice: "", style: "" } }));
      onChanged();
    } catch (error) {
      notifyError(error);
    } finally {
      setBusy(null);
    }
  };

  if (detail.roles.length === 0) {
    return <span className="muted tiny">暂无角色（先导入对白脚本）</span>;
  }

  return (
    <div className="stack">
      {detail.roles.map((role) => {
        const draft = draftOf(role.name);
        return (
          <div className="row" key={role.name}>
            <Tag size="small" variant={role.bound ? "solid" : "outlined"} color={role.bound ? "app-teal" : "default"}>
              {role.name}
            </Tag>
            <span className="tiny muted">
              {role.bound ? `音色：${role.voice ?? "-"}` : "未绑定"}
            </span>
            <Input
              size="small"
              style={{ width: 240 }}
              placeholder="音色（如 冰糖 / 雨夜女主-固化）"
              value={draft.voice}
              onChange={(event) =>
                setDrafts((prev) => ({
                  ...prev,
                  [role.name]: { ...draft, voice: event.target.value },
                }))
              }
            />
            <Input
              size="small"
              style={{ width: 280 }}
              placeholder="默认风格指令（可选）"
              value={draft.style}
              onChange={(event) =>
                setDrafts((prev) => ({
                  ...prev,
                  [role.name]: { ...draft, style: event.target.value },
                }))
              }
            />
            <Button size="small" loading={busy === role.name} onClick={() => void bind(role.name)}>
              绑定
            </Button>
          </div>
        );
      })}
    </div>
  );
}

function ExportPanel({
  detail,
  onClose,
}: {
  detail: ProjectDetail;
  onClose: () => void;
}) {
  const [format, setFormat] = useState("wav");
  const [layout, setLayout] = useState("role");
  const [concat, setConcat] = useState(false);
  const [normalize, setNormalize] = useState(false);
  const [trimSilence, setTrimSilence] = useState(false);
  const [nameTemplate, setNameTemplate] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ExportResult | null>(null);

  const run = async () => {
    setBusy(true);
    try {
      const outcome = await api.exportRun(detail.path, {
        format,
        layout,
        concat,
        normalize,
        trimSilence,
        nameTemplate: nameTemplate.trim() ? nameTemplate.trim() : null,
      });
      setResult(outcome);
      notifySuccess(`已导出 ${outcome.count} 行 → ${outcome.out_dir}`);
    } catch (error) {
      notifyError(error);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card color="app-yellow" pattern="default">
      <div className="stack">
        <div className="row-between">
          <strong>导出</strong>
          <Button size="small" type="text" onClick={onClose}>
            收起
          </Button>
        </div>
        <div className="row">
          <Select
            options={[
              { key: "wav", label: "wav（无损，直复制）" },
              { key: "mp3", label: "mp3（192k，需 ffmpeg）" },
            ]}
            value={format}
            onChange={setFormat}
          />
          <Select
            options={[
              { key: "flat", label: "布局：平铺" },
              { key: "role", label: "布局：按角色" },
              { key: "scene", label: "布局：按场景" },
            ]}
            value={layout}
            onChange={setLayout}
          />
          <Input
            size="small"
            style={{ width: 220 }}
            placeholder="文件名模板（可选）"
            value={nameTemplate}
            onChange={(event) => setNameTemplate(event.target.value)}
          />
        </div>
        <div className="row">
          <span className="tiny">整轨拼接</span>
          <Switch checked={concat} onChange={setConcat} size="small" />
          <span className="tiny">响度归一化</span>
          <Switch checked={normalize} onChange={setNormalize} size="small" />
          <span className="tiny">静音裁剪</span>
          <Switch checked={trimSilence} onChange={setTrimSilence} size="small" />
          <Button size="small" type="primary" loading={busy} onClick={() => void run()}>
            开始导出
          </Button>
        </div>
        {result ? (
          <div className="stack">
            <DividerLine />
            <div className="tiny mono">{result.out_dir}</div>
            <div className="row">
              <Button size="small" onClick={() => void api.revealPath(result.out_dir).catch(notifyError)}>
                在 Finder 中显示
              </Button>
              <span className="tiny muted">
                {result.count} 个文件 · 清单 manifest.json
                {result.concat ? " · 含整轨 full." + result.format : ""}
              </span>
            </div>
          </div>
        ) : null}
      </div>
    </Card>
  );
}

function DividerLine() {
  return <div style={{ borderTop: "1px dashed rgba(0,0,0,0.18)" }} />;
}

export default function ProjectsPage() {
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [detail, setDetail] = useState<ProjectDetail | null>(null);
  const [busy, setBusy] = useState(false);
  const [newPath, setNewPath] = useState("");
  const [newName, setNewName] = useState("");
  const [scriptPath, setScriptPath] = useState("");
  const [showExport, setShowExport] = useState(false);
  const [concurrency, setConcurrency] = useState("");
  const [force, setForce] = useState(false);
  const [events, setEvents] = useState<BatchEvent[]>([]);
  const [batchRunning, setBatchRunning] = useState(false);
  const [batchSummary, setBatchSummary] = useState<string | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [showRoles, setShowRoles] = useState(false);
  const logRef = useRef<HTMLDivElement | null>(null);

  const refreshProjects = useCallback(async () => {
    try {
      const payload = await api.projectList();
      setProjects(payload.projects);
      setSelected((current) => current ?? payload.projects[0]?.path ?? null);
    } catch (error) {
      notifyError(error);
    }
  }, []);

  const refreshDetail = useCallback(async () => {
    if (!selected) {
      setDetail(null);
      return;
    }
    try {
      setDetail(await api.projectDetail(selected));
    } catch (error) {
      notifyError(error);
    }
  }, [selected]);

  useEffect(() => {
    void refreshProjects();
  }, [refreshProjects]);

  useEffect(() => {
    void refreshDetail();
  }, [refreshDetail]);

  useEffect(() => {
    if (logRef.current) {
      logRef.current.scrollTop = logRef.current.scrollHeight;
    }
  }, [events]);

  const batchPercent = useMemo(() => {
    const last = events[events.length - 1];
    if (!last) return 0;
    if (last.event === "finished") return 100;
    if (last.event === "started") return 0;
    const started = events.find((event) => event.event === "started");
    const total = started && started.event === "started" ? started.total : 0;
    if (total === 0) return 0;
    const done = events.filter(
      (event) => event.event === "line_succeeded" || event.event === "line_failed",
    ).length;
    return Math.min(100, Math.round((done / total) * 100));
  }, [events]);

  const createProject = async () => {
    if (!newPath.trim()) {
      notificationOpen({ message: "提示", description: "请填写项目目录", type: "warning" });
      return;
    }
    setBusy(true);
    try {
      const created = await api.projectInit(newPath.trim(), newName.trim());
      notifySuccess(`项目「${created.name}」已创建`);
      setNewPath("");
      setNewName("");
      setCreateOpen(false);
      await refreshProjects();
      setSelected(created.path);
    } catch (error) {
      notifyError(error);
    } finally {
      setBusy(false);
    }
  };

  const importScript = async () => {
    if (!detail || !scriptPath.trim()) {
      notificationOpen({ message: "提示", description: "请填写脚本文件路径", type: "warning" });
      return;
    }
    setBusy(true);
    try {
      const outcome = await api.projectImport(detail.path, scriptPath.trim());
      notifySuccess(
        `导入 ${outcome.counts.lines} 条（新增 ${outcome.diff.added} / 修改 ${outcome.diff.changed} / 移除 ${outcome.diff.removed}），未绑定角色：${
          outcome.unbound_roles.join("、") || "无"
        }`,
      );
      setScriptPath("");
      await refreshDetail();
      await refreshProjects();
    } catch (error) {
      notifyError(error);
    } finally {
      setBusy(false);
    }
  };

  const runBatch = async () => {
    if (!detail) return;
    setBatchRunning(true);
    setEvents([]);
    setBatchSummary(null);
    const channel = new Channel<BatchEvent>();
    channel.onmessage = (event) => setEvents((prev) => [...prev, event]);
    try {
      const parsed = concurrency.trim() ? Number.parseInt(concurrency.trim(), 10) : null;
      const result = await api.batchRun(
        detail.path,
        { concurrency: parsed && parsed > 0 ? parsed : null, force },
        channel,
      );
      setBatchSummary(
        `成功 ${result.summary.succeeded} / 失败 ${result.summary.failed} / 跳过 ${result.summary.skipped}（缓存 ${result.summary.cached}）`,
      );
      notifySuccess("批量生成完成");
    } catch (error) {
      if (isPartialFailure(error)) {
        notificationOpen({
          message: "批量部分失败",
          description: error instanceof Error ? error.message : String(error),
          type: "warning",
        });
      } else {
        notifyError(error);
      }
    } finally {
      setBatchRunning(false);
      await refreshDetail();
      await refreshProjects();
    }
  };

  return (
    <div className="grid-two-wide">
      <div className="stack">
        <Card color="app-teal" pattern="default">
          <div className="stack">
            <div className="row-between">
              <strong>项目</strong>
              <div className="row">
                <Button size="small" onClick={() => void refreshProjects()}>
                  刷新
                </Button>
                <Button size="small" type="primary" onClick={() => setCreateOpen(true)}>
                  新建
                </Button>
              </div>
            </div>
            {projects.length === 0 ? (
              <span className="muted tiny">还没有项目，点击「新建」开始</span>
            ) : (
              projects.map((project) => (
                <div
                  key={project.path}
                  className={`project-list-item${selected === project.path ? " active" : ""}`}
                  onClick={() => setSelected(project.path)}
                >
                  <strong>{project.name}</strong>
                  <span className="tiny muted">
                    {`${project.counts.total} 条 · 已生成 ${project.counts.ok} · 失败 ${project.counts.failed}`}
                  </span>
                  <span className="tiny mono muted">{project.path}</span>
                </div>
              ))
            )}
          </div>
        </Card>

        <Modal
          open={createOpen}
          title="新建项目"
          onClose={() => setCreateOpen(false)}
          onOk={() => void createProject()}
          footer={
            <div className="row">
              <Button size="small" onClick={() => setCreateOpen(false)}>
                取消
              </Button>
              <Button size="small" type="primary" loading={busy} onClick={() => void createProject()}>
                创建
              </Button>
            </div>
          }
        >
          <div className="stack">
            <Input
              placeholder="项目目录（绝对路径，如 ~/.tts-workbench/projects/我的作品）"
              value={newPath}
              onChange={(event) => setNewPath(event.target.value)}
            />
            <Input
              placeholder="作品名（可选）"
              value={newName}
              onChange={(event) => setNewName(event.target.value)}
            />
          </div>
        </Modal>
      </div>

      <div className="stack">
        {!detail ? (
          <Card color="default">
            <span className="muted">请选择或新建一个项目</span>
          </Card>
        ) : (
          <>
            <Card color="default" pattern="none">
              <div className="stack">
                <div className="row-between">
                  <div className="row">
                    <strong>{detail.name}</strong>
                    <Tag size="small" variant="soft">
                      {`共 ${detail.counts.total}`}
                    </Tag>
                    <Tag size="small" variant="soft" color="app-green">
                      {`已生成 ${detail.counts.ok}`}
                    </Tag>
                    <Tag size="small" variant="soft" color="app-red">
                      {`失败 ${detail.counts.failed}`}
                    </Tag>
                    <Tag size="small" variant="soft" color="brown">
                      {`移除 ${detail.counts.removed}`}
                    </Tag>
                  </div>
                  <div className="row">
                    <Button size="small" onClick={() => void refreshDetail()}>
                      刷新
                    </Button>
                    <Button size="small" onClick={() => setShowExport((value) => !value)}>
                      导出
                    </Button>
                    <Button
                      size="small"
                      type="primary"
                      loading={batchRunning}
                      onClick={() => void runBatch()}
                    >
                      批量生成
                    </Button>
                  </div>
                </div>
                <span className="tiny mono muted">{detail.path}</span>
                <div className="row">
                  <Input
                    size="small"
                    style={{ flex: 1, minWidth: 280 }}
                    placeholder="对白脚本路径（导入 / 重新导入）"
                    value={scriptPath}
                    onChange={(event) => setScriptPath(event.target.value)}
                  />
                  <Button size="small" loading={busy} onClick={() => void importScript()}>
                    导入脚本
                  </Button>
                </div>
                <div className="row">
                  <Input
                    size="small"
                    style={{ width: 120 }}
                    placeholder="并发数"
                    value={concurrency}
                    onChange={(event) => setConcurrency(event.target.value)}
                  />
                  <span className="tiny">强制重生成</span>
                  <Switch size="small" checked={force} onChange={setForce} />
                </div>
                {batchRunning || events.length > 0 ? (
                  <div className="stack">
                    <Progress
                      percent={batchPercent}
                      size="small"
                      showInfo
                      infoFormat={(percent) => `${percent}%`}
                    />
                    {batchSummary ? <span className="tiny">本次：{batchSummary}</span> : null}
                    <div className="batch-progress" ref={logRef}>
                      {events.map((event, index) => (
                        <div key={index} className="mono tiny">
                          {summarizeEvent(event)}
                        </div>
                      ))}
                    </div>
                  </div>
                ) : null}
              </div>
            </Card>

            {showExport ? <ExportPanel detail={detail} onClose={() => setShowExport(false)} /> : null}

            <Card color="app-blue" pattern="none">
              <div className="stack">
                <div className="row-between">
                  <strong>{`角色绑定（${detail.roles.length}）`}</strong>
                  <Button size="small" onClick={() => setShowRoles((value) => !value)}>
                    {showRoles ? "收起" : "展开"}
                  </Button>
                </div>
                {showRoles ? (
                  <RoleBindingTable detail={detail} onChanged={() => void refreshDetail()} />
                ) : (
                  <span className="tiny">
                    {detail.roles
                      .map((role) => `${role.name}：${role.bound ? (role.voice ?? "已绑定") : "未绑定"}`)
                      .join(" · ") || "暂无角色"}
                  </span>
                )}
              </div>
            </Card>

            <div className="stack">
              {detail.lines
                .filter((line) => line.status !== "removed")
                .map((line) => (
                  <LineCard
                    key={line.id}
                    line={line}
                    projectPath={detail.path}
                    onChanged={() => void refreshDetail()}
                  />
                ))}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
