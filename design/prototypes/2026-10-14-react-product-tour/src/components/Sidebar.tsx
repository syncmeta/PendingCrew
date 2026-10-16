import { useTour } from "../state/TourContext";
import type { Scene } from "../domain/types";
import { Button, Icon, Lineage } from "./primitives";
import { ResizeHandle } from "./ResizeHandle";
function CrewRow({
  scene,
  indent = 0,
  alias = false,
}: {
  scene: Scene;
  indent?: number;
  alias?: boolean;
}) {
  const { state, selectScene } = useTour();
  return (
    <Button
      action="scene"
      data-id={scene.id}
      className={"crew-row " + (scene.id === state.sceneId ? "selected" : "")}
      style={{ paddingLeft: indent + 8 }}
      onClick={() => selectScene(scene.id)}
    >
      <span className="chevron">{scene.id === "trip" ? "▾" : ""}</span>
      <Lineage id={scene.id} />
      <span className="crew-text">
        <span className="crew-top">
          <span>
            {scene.crew}
            {alias ? " ↗" : ""}
          </span>
          <small>{scene.id === state.sceneId ? "刚刚" : "已整理"}</small>
        </span>
        <span className="crew-sub">
          {alias ? "同一机组 · 另一父引用" : scene.note}
        </span>
      </span>
    </Button>
  );
}
export function Sidebar() {
  const {
    state,
    data: { ui },
    setUi,
    setModal,
    dispatch,
  } = useTour();
  return (
    <aside className="sidebar">
      <div className="side-toolbar">
        <div className="lights" aria-label="Mac 窗口装饰">
          <i />
          <i />
          <i />
        </div>
        <div className="tools">
          <Button
            action="search"
            className="icon-button"
            aria-label="搜索介绍内容"
            title="搜索介绍内容"
            onClick={() => setModal({ kind: "search" })}
          >
            <Icon name="search" />
          </Button>
          <Button
            action="new-crew"
            className="icon-button"
            aria-label="新建机组"
            title="新建机组"
            onClick={() => setModal({ kind: "newCrew" })}
          >
            <Icon name="plus" />
          </Button>
          <Button
            action="refresh"
            className="icon-button"
            aria-label="刷新"
            title="刷新"
            onClick={() =>
              dispatch({ type: "notice", text: "当前机组状态已刷新。" })
            }
          >
            <Icon name="refresh" />
          </Button>
        </div>
      </div>
      <div className="segmented side-mode">
        {(["hierarchy", "timeline"] as const).map((mode) => (
          <Button
            key={mode}
            action="sidebar"
            data-mode={mode}
            className={ui.sidebar === mode ? "active" : ""}
            onClick={() => setUi({ sidebar: mode })}
          >
            {mode === "hierarchy" ? "通讯录" : "消息"}
          </Button>
        ))}
      </div>
      <div className="side-list">
        {ui.sidebar === "hierarchy" ? (
          <>
            <Button
              action="machine"
              className="machine"
              onClick={() => setUi({ machineClosed: !ui.machineClosed })}
            >
              {ui.machineClosed ? "▸" : "▾"} 本机
            </Button>
            {!ui.machineClosed && (
              <>
                <div className="crew-row">
                  <span className="chevron">▾</span>
                  <Lineage id="crew-product" />
                  <span className="crew-text">
                    <span className="crew-top">PendingCrew</span>
                    <span className="crew-sub">
                      为持续任务流设计的人机协作工具
                    </span>
                  </span>
                </div>
                {state.scenes.map((s) => (
                  <CrewRow
                    key={s.id}
                    scene={s}
                    indent={s.id === "research" ? 30 : 15}
                  />
                ))}
                <div className="crew-row">
                  <span className="chevron">▾</span>
                  <Lineage id="crew-reading" />
                  <span className="crew-text">
                    <span className="crew-top">协作网络</span>
                    <span className="crew-sub">机组可以连接多个父机组</span>
                  </span>
                </div>
                <CrewRow scene={state.scenes[2]} indent={15} alias />
              </>
            )}
          </>
        ) : (
          state.scenes
            .slice()
            .sort((a, b) =>
              a.id === state.sceneId ? -1 : b.id === state.sceneId ? 1 : 0,
            )
            .map((s) => <CrewRow key={s.id} scene={s} />)
        )}
      </div>
      <div className="side-footer">
        <Button
          action="hidden"
          className=""
          onClick={() =>
            dispatch({ type: "notice", text: "这份介绍里没有隐藏的机组。" })
          }
        >
          已隐藏的群 ▸
        </Button>
        <div className="quota">
          <span className="quota-ring" />
          Codex · 额度
        </div>
        <span>本机 · 产品介绍</span>
      </div>
      <ResizeHandle name="sidebar" />
    </aside>
  );
}
