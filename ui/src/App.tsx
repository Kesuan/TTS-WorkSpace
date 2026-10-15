import { useEffect, useMemo, useState } from "react";

import { Background, Tabs, Tag, Title, Tooltip } from "animal-island-ui";
import {
  BookIcon,
  CreditCardIcon,
  FolderIcon,
  MicIcon,
  SettingsIcon,
} from "naive-icons";

import { api } from "./api";
import ProjectsPage from "./pages/ProjectsPage";
import SettingsPage from "./pages/SettingsPage";
import UsagePage from "./pages/UsagePage";
import VoicesPage from "./pages/VoicesPage";
import type { AppInfo } from "./types";

const ICON_SIZE = 16;

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
      {
        key: "projects",
        label: (
          <span className="row" style={{ gap: 6, flexWrap: "nowrap" }}>
            <BookIcon size={ICON_SIZE} />
            项目
          </span>
        ),
        children: <ProjectsPage />,
      },
      {
        key: "voices",
        label: (
          <span className="row" style={{ gap: 6, flexWrap: "nowrap" }}>
            <MicIcon size={ICON_SIZE} />
            音色库
          </span>
        ),
        children: <VoicesPage />,
      },
      {
        key: "usage",
        label: (
          <span className="row" style={{ gap: 6, flexWrap: "nowrap" }}>
            <CreditCardIcon size={ICON_SIZE} />
            用量
          </span>
        ),
        children: <UsagePage />,
      },
      {
        key: "settings",
        label: (
          <span className="row" style={{ gap: 6, flexWrap: "nowrap" }}>
            <SettingsIcon size={ICON_SIZE} />
            设置
          </span>
        ),
        children: <SettingsPage />,
      },
    ],
    [],
  );

  return (
    <Background type="default" className="app-bg">
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
                <Tooltip title={info.workspace_root} placement="bottom-end">
                  <span className="app-workspace mono">
                    <FolderIcon size={14} style={{ flex: "none" }} />
                    <span className="app-workspace-path">{info.workspace_root}</span>
                  </span>
                </Tooltip>
              </>
            ) : (
              <span className="muted">{error ?? "加载中…"}</span>
            )}
          </div>
        </div>
        <div className="app-body">
          <Tabs items={items} defaultActiveKey="projects" shadow aria-label="主导航" />
        </div>
      </div>
    </Background>
  );
}
