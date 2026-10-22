import type { CSSProperties } from "react";
import { TourProvider, useTour } from "./state/TourContext";
import { type Scope } from "./domain/types";
import { Button, Icon } from "./components/primitives";
import { Sidebar } from "./components/Sidebar";
import { ConversationList } from "./components/ConversationList";
import { Chat } from "./components/Chat";
import { Workspace } from "./components/Workspace";
import { Dialogs } from "./components/Dialogs";
import "./styles/pendingcrew.css";
export interface PendingCrewTourProps {
  /** Standalone shows the demo and reset control; embedded shows only the Mac interface. */
  mode?: "standalone" | "embedded";
  height?: CSSProperties["height"];
  className?: string;
  /** Use different storage keys for different products or instances. */
  storageKey?: string;
  persist?: boolean;
  initialChapter?: string;
  migrateLegacy?: boolean;
}
function MacWindow() {
  const {
    state,
    dispatch,
    data: { ui },
    scene,
    setUi,
    setModal,
    sidebarOpen,
    setSidebarOpen,
  } = useTour();
  return (
    <main
      className={"mac-window " + (sidebarOpen ? "show-sidebar " : "") + (state.navigation?.sidebarCollapsed ? "sidebar-collapsed" : "")}
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
            action="toggle-sidebar"
            className="icon-button sidebar-toggle"
            aria-label={state.navigation?.sidebarCollapsed ? "展开侧栏" : "收起侧栏"}
            onClick={() => window.innerWidth <= 760 ? setSidebarOpen(!sidebarOpen) : dispatch({type:"toggleSidebar"})}
          >
            <Icon name="panel" />
          </Button>
          <h2>{scene.crew}</h2>
          <span className="path" data-current-crew={scene.crew}>
            本机
          </span>
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
              aria-label={ui.tab === "todo" ? "Todo 标签" : "管理分组"}
              onClick={() => ui.tab === "todo" ? setUi({ tab: "todo", todoTag: "all" }) : setModal({ kind: "folders", scope: ui.tab })}
            >
              <Icon name="more" />
            </Button>
          </div>
        </div>
        <div className={"group-layout " + (ui.mobileChat ? "mobile-chat" : "")}>
          <ConversationList />
          <div className="group-detail">
            <nav className="view-tabs" aria-label="当前群内容">
              {([
                ["message","消息"],["human-todo","真人Todo"],["ai-todo","AI Todo"],["document","文档/数据"],["progress","计划/进展"]
              ] as const).map(([key,label]) => {
                const todo=key==="human-todo"||key==="ai-todo";
                const kind=key==="human-todo"?"asks":"requests";
                const active=todo?ui.tab==="todo"&&(ui.kind==="asks"?"asks":"requests")===kind:ui.tab===key;
                return <Button key={key} action="tab" data-tab={key} className={active?"active":""} aria-pressed={active} onClick={()=>{
                  const filters={...ui.todoFilters};
                  if(ui.tab==="todo") filters[ui.kind==="asks"?"asks":"requests"]={tag:ui.todoTag??"all",query:ui.query};
                  setUi(todo?{tab:"todo",kind,todoTag:filters[kind]?.tag??"all",query:filters[kind]?.query??"",todoFilters:filters,doc:null}:{tab:key as Scope,query:"",doc:null,todoFilters:filters});
                }}>{label}</Button>;
              })}
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
      <MacWindow />
      <footer className="demo-footer">
        <nav className="web-product-links" aria-label="产品链接">
          <a href="https://github.com/syncmeta/PendingCrew" target="_blank" rel="noopener noreferrer">GitHub</a>
          <a href="https://docs.pendingname.com/pendingcrew/" target="_blank" rel="noopener noreferrer">文档</a>
          <a href="https://pending.name" target="_blank" rel="noopener noreferrer">个人网站</a>
        </nav>
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
