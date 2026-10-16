import type { CSSProperties } from "react";
import { TourProvider, useTour } from "./state/TourContext";
import { scopeNames, type Scope } from "./domain/types";
import { Button, Icon } from "./components/primitives";
import { Sidebar } from "./components/Sidebar";
import { ConversationList } from "./components/ConversationList";
import { Chat } from "./components/Chat";
import { Workspace } from "./components/Workspace";
import { Dialogs } from "./components/Dialogs";
import "./styles/pendingcrew.css";
export interface PendingCrewTourProps {
  /** Standalone includes the website navigation; embedded includes only the Mac interface. */
  mode?: "standalone" | "embedded";
  height?: CSSProperties["height"];
  className?: string;
  /** Use different storage keys for different products or instances. */
  storageKey?: string;
  persist?: boolean;
  initialChapter?: string;
  migrateLegacy?: boolean;
}
function WebsiteHeader() {
  const { state, selectScene, setModal } = useTour();
  return (
    <header className="intro">
      <a
        className="brand"
        href="#"
        onClick={(e) => {
          e.preventDefault();
          selectScene("website");
        }}
      >
        PendingCrew
      </a>
      <nav aria-label="网站导航">
        {[
          ["website", "介绍"],
          ["trip", "协作方式"],
          ["research", "组织与沟通"],
        ].map(([id, label]) => (
          <Button
            key={id}
            action="scene"
            data-id={id}
            className=""
            onClick={() => selectScene(id)}
          >
            {label}
          </Button>
        ))}
        <a
          href="https://docs.pendingname.com/pendingcrew"
          target="_blank"
          rel="noopener"
        >
          文档 ↗
        </a>
      </nav>
      <select
        id="scenario"
        aria-label="介绍章节"
        hidden
        value={state.sceneId}
        onChange={(e) => selectScene(e.target.value)}
      >
        {state.scenes.map((s) => (
          <option key={s.id} value={s.id}>
            {s.name}
          </option>
        ))}
      </select>
      <Button id="guide" hidden onClick={() => setModal({ kind: "guide" })}>
        产品结构
      </Button>
    </header>
  );
}
function MacWindow() {
  const {
    data: { ui },
    scene,
    setUi,
    setModal,
    sidebarOpen,
    setSidebarOpen,
  } = useTour();
  return (
    <main
      className={"mac-window " + (sidebarOpen ? "show-sidebar" : "")}
      id="app"
      aria-label="PendingCrew Mac 网页介绍"
    >
      <Sidebar />
      <Button
        className="sidebar-scrim"
        action="close-sidebar"
        aria-label="收起机组列表"
        onClick={() => setSidebarOpen(false)}
      />
      <section className="detail">
        <div className="titlebar">
          <Button
            action="mobile-menu"
            className="icon-button mobile-menu"
            aria-label="显示机组列表"
            onClick={() => setSidebarOpen(!sidebarOpen)}
          >
            <Icon name="menu" />
          </Button>
          <h2 title={scene.crew}>{scene.crew}</h2>
          <span className="path">{scene.root} / 本机</span>
          <span className="app-notice">交互介绍</span>
          <label className="search">
            <Icon name="search" />
            <input
              aria-label="搜索当前群"
              placeholder="搜索当前群"
              value={ui.query}
              id="search-input"
              onChange={(e) => setUi({ query: e.target.value })}
            />
          </label>
          <div className="tools">
            <Button
              action="guide"
              className="icon-button"
              aria-label="了解这份介绍"
              onClick={() => setModal({ kind: "guide" })}
            >
              <Icon name="info" />
            </Button>
            <Button
              action="folder-manager"
              className="icon-button"
              aria-label="管理分组"
              onClick={() => setModal({ kind: "folders", scope: ui.tab })}
            >
              <Icon name="more" />
            </Button>
          </div>
        </div>
        <div className={"group-layout " + (ui.mobileChat ? "mobile-chat" : "")}>
          <ConversationList />
          <div className="group-detail">
            <nav className="view-tabs" aria-label="当前群内容">
              {(Object.entries(scopeNames) as [Scope, string][]).map(
                ([tab, label]) => (
                  <Button
                    key={tab}
                    action="tab"
                    data-tab={tab}
                    className={ui.tab === tab ? "active" : ""}
                    aria-pressed={ui.tab === tab}
                    onClick={() => setUi({ tab, query: "", doc: null })}
                  >
                    {label}
                  </Button>
                ),
              )}
            </nav>
            <section
              className={"view " + (ui.mobileChat ? "mobile-chat" : "")}
              id="main-view"
            >
              {ui.tab === "message" ? <Chat /> : <Workspace />}
            </section>
          </div>
        </div>
      </section>
    </main>
  );
}
function TourView({
  mode,
  height,
  className,
}: {
  mode: "standalone" | "embedded";
  height?: CSSProperties["height"];
  className?: string;
}) {
  const { state, setModal } = useTour();
  return (
    <div
      className={
        "pc-tour " + (mode === "embedded" ? "embed " : "") + (className ?? "")
      }
      style={mode === "embedded" ? { height: height ?? 800 } : undefined}
    >
      <WebsiteHeader />
      <MacWindow />
      <footer className="demo-footer">
        <span className="status-dot" />
        可交互的产品介绍 · 不连接真实 agent{" "}
        <Button
          id="reset"
          className="footer-right"
          onClick={() => setModal({ kind: "reset" })}
        >
          重置 Demo
        </Button>
      </footer>
      <Dialogs />
      {state.notice && (
        <div className="toast" id="toast" role="status" aria-live="polite">
          {state.notice.text}
        </div>
      )}
    </div>
  );
}
export function PendingCrewTour({
  mode = "standalone",
  height,
  className,
  storageKey = "pendingcrew-react-tour-v2",
  persist = true,
  initialChapter = "website",
  migrateLegacy = true,
}: PendingCrewTourProps) {
  return (
    <TourProvider
      storageKey={storageKey}
      persist={persist}
      initialChapter={initialChapter}
      migrateLegacy={migrateLegacy}
    >
      <TourView mode={mode} height={height} className={className} />
    </TourProvider>
  );
}
