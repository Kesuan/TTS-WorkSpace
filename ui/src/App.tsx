import { useEffect, useMemo, useState } from "react";

import { Tabs, Tag, Title } from "animal-island-ui";

import { api } from "./api";
import ProjectsPage from "./pages/ProjectsPage";
import SettingsPage from "./pages/SettingsPage";
import UsagePage from "./pages/UsagePage";
import VoicesPage from "./pages/VoicesPage";
import type { AppInfo } from "./types";

export default function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .appInfo()
      .then(setInfo)
      .catch((reason: unknown) => {
        setError(reason instanceof Error ? reason.message : String(reason));
      });
  }, []);

  const items = useMemo(
    () => [
      { key: "projects", label: "项目", children: <ProjectsPage /> },
      { key: "voices", label: "音色库", children: <VoicesPage /> },
      { key: "usage", label: "用量", children: <UsagePage /> },
      { key: "settings", label: "设置", children: <SettingsPage /> },
    ],
    [],
  );

  return (
    <div className="app-shell">
      <div className="app-topbar">
        <Title size="middle" color="app-teal">
          TTS WorkSpace
        </Title>
        <div className="app-topbar-meta">
          {info ? (
            <>
              <Tag size="small" variant="soft" color="app-blue">
                {`v${info.version}`}
              </Tag>
              <span className="mono">{info.workspace_root}</span>
            </>
          ) : (
            <span className="muted">{error ?? "加载中…"}</span>
          )}
        </div>
      </div>
      <div className="app-body">
        <Tabs items={items} defaultActiveKey="projects" aria-label="主导航" />
      </div>
    </div>
  );
}
