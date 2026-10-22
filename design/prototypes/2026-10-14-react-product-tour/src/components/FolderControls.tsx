import { useTour } from "../state/TourContext";
import { scopeNames, type Scope } from "../domain/types";
import { Button, Icon } from "./primitives";
export function FolderSelect({ scope }: { scope: Scope }) {
  const {
    data: { db, ui },
    setUi,
  } = useTour();
  return (
    <select
      aria-label={scopeNames[scope] + "文件夹"}
      data-folder-scope={scope}
      value={ui.selectedFolders[scope]}
      onChange={(e) =>
        setUi({
          selectedFolders: { ...ui.selectedFolders, [scope]: e.target.value },
        })
      }
    >
      <option value="all">全部{scopeNames[scope]}</option>
      {[...db.folders[scope], "未分组"].map((f) => (
        <option key={f}>{f}</option>
      ))}
    </select>
  );
}
export function FolderRail() {
  const {
    state,
    selectScene,
    data: { db, ui },
    setUi,
    setModal,
  } = useTour();
  const scope = ui.tab;
  if(scope === "progress" && ["website","current-progress"].includes(state.sceneId) && ui.conv.endsWith("-main")) return <aside className="folder-rail"><div className="folder-head">计划</div><Button className="folder-button selected" onClick={()=>setUi({selectedFolders:{...ui.selectedFolders,progress:"all"}})}><Icon name="folder"/>全部计划/进展</Button><Button className="folder-button" onClick={()=>{selectScene("website");setUi({conv:"website-program",tab:"progress",query:"",selectedFolders:{...ui.selectedFolders,progress:"all"}});}}><Icon name="folder"/>大工程</Button></aside>;
  return (
    <aside className="folder-rail">
      <div className="folder-head">
        文件夹{" "}
        <Button
          action="new-folder"
          className="icon-button"
          aria-label="新建文件夹"
          onClick={() => setModal({ kind: "folderEdit", scope })}
        >
          <Icon name="plus" />
        </Button>
      </div>
      {["all", ...db.folders[scope], "未分组"].map((f) => (
        <Button
          key={f}
          action="select-folder"
          data-folder={f}
          className={
            "folder-button " +
            (ui.selectedFolders[scope] === f ? "selected" : "")
          }
          onClick={() =>
            setUi({
              selectedFolders: { ...ui.selectedFolders, [scope]: f },
              doc: null,
            })
          }
        >
          <Icon name="folder" />
          {f === "all" ? "全部" + scopeNames[scope] : f}
        </Button>
      ))}
      <div className="folder-bottom">
        <Button
          action="folder-manager"
          onClick={() => setModal({ kind: "folders", scope })}
        >
          管理分组
        </Button>
      </div>
    </aside>
  );
}
