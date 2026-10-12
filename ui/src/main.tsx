import React from "react";
import ReactDOM from "react-dom/client";

// animal-island-ui 全局样式（必须引入，否则组件无样式）
import "animal-island-ui/style";
import "./styles.css";

import App from "./App";

const root = document.getElementById("root");
if (!root) {
  throw new Error("缺少 #root 挂载点");
}

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
