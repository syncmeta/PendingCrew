export type Scope = "message" | "todo" | "document" | "progress";
export type Status = "open" | "active" | "blocked" | "done" | "dropped";
export type TodoKind = "requests" | "plans" | "asks";
export interface Scene {
  id: string;
  name: string;
  crew: string;
  root: string;
  brief: string;
  project: string;
  request: string;
  question: string;
  recommendation: string;
  options: string[];
  folderNames: Record<Scope, string[]>;
  steps: string[];
  leaf: string;
  docTitle: string;
  docBody: string;
  agent: string;
  glyph: string;
  note: string;
  intro: string;
  initialLedger?: Ledger;
}
export interface Classified {
  id: string;
  folder?: string;
}
export interface Plan extends Classified {
  code: string;
  title: string;
  parent: string | null;
  status: Status;
  owner: string;
  sources: string[];
  dependsOn: string[];
  blockedOn: string[];
  nextCheck: string;
  note: string;
  goal?: string;
  level?: string;
  acceptance?: string;
  evidence?: string;
}
export interface Request extends Classified {
  code: string;
  text: string;
}
export interface HumanAsk extends Classified {
  question: string;
  recommendation: string;
  options: string[];
  status: "open" | "later" | "answered" | "withdrawn";
  answer: string;
  item: string;
  context: string;
}
export interface TableDataView {
  kind: "table";
  primaryKey: string;
  description: string;
  schema: string;
  columns: { key: string; label: string; type: string }[];
  rows: Record<string, string | null | string[]>[];
}
export interface CompositionDataView {
  kind: "composition";
  description: string;
  blocks: { id: string; title: string; body: string; source: string }[];
  views: {
    id: string;
    title: string;
    placements: { id: string; blockId: string }[];
  }[];
}
export type DataView = TableDataView | CompositionDataView;
export interface DocumentRecord extends Classified {
  dataView?: DataView;
  catalogKey?: string;
  summary?: string;
  kind?: "产品说明" | "已敲定设计" | "规划设想" | "工程说明";
  sources?: { path: string; label: string }[];
  related?: string[];
  title: string;
  body: string;
  author: string;
}
export interface Conversation {
  id: string;
  title: string;
  kind: "group" | "dm";
  subtitle: string;
}
export interface Message extends Classified {
  sender: string;
  text: string;
  ref?: string | null;
  internal?: boolean;
  link?: { label: string; href: string };
}
export interface Ledger {
  plans: Plan[];
  requests: Request[];
  asks: HumanAsk[];
  docs: DocumentRecord[];
  conversations: Conversation[];
  messages: Record<string, Message[]>;
  folders: Record<Scope, string[]>;
  placements: Record<string, string>;
  history: { text: string; time: string }[];
}
export interface UiState {
  tab: Scope;
  kind: TodoKind;
  conv: string;
  sidebar: "hierarchy" | "timeline";
  collapsed: string[];
  selectedFolders: Record<Scope, string>;
  inspector: boolean;
  todoClosed: boolean;
  membersClosed: boolean;
  drafts: Record<string, string>;
  scrolls: Record<string, number>;
  doc: string | null;
  mentions: boolean;
  query: string;
  mobileChat: boolean;
  machineClosed: boolean;
  mode: "human" | "all";
  askScope: "crew" | "all";
  counter: number;
}
export type GroupWork = Pick<
  Ledger,
  "plans" | "requests" | "asks" | "docs" | "folders" | "placements" | "history"
>;
export interface SceneState {
  groups?: Record<string, GroupWork>;
  db: Ledger;
  ui: UiState;
}
export interface TourState {
  contentRevision?: number;
  sceneId: string;
  scenes: Scene[];
  cache: Record<string, SceneState>;
  notice: { id: number; text: string } | null;
}
export type ModalState =
  | { kind: "request"; id: string }
  | { kind: "plan"; id: string }
  | { kind: "ask"; id: string }
  | { kind: "move"; id: string; scope: Scope }
  | { kind: "folderEdit"; scope: Scope; old?: string }
  | { kind: "folders"; scope: Scope }
  | {
      kind:
        | "search"
        | "guide"
        | "history"
        | "newCrew"
        | "newRequest"
        | "newDoc"
        | "references"
        | "reset";
    }
  | null;
export type Command =
  | { type: "restore"; state: TourState }
  | { type: "selectScene"; id: string }
  | { type: "ui"; patch: Partial<UiState> }
  | { type: "send"; text: string; ref?: string }
  | { type: "shareReference"; id: string }
  | { type: "foldPlan"; id: string }
  | { type: "answer"; id: string; text: string; sceneId?: string }
  | { type: "askLater" | "askWithdraw"; id: string; sceneId?: string }
  | {
      type: "savePlan";
      id: string;
      status: Status;
      owner: string;
      note: string;
    }
  | { type: "saveFolder"; scope: Scope; name: string; old?: string }
  | { type: "move"; id: string; scope: Scope; folder: string }
  | { type: "newRequest"; text: string }
  | { type: "newDocument"; title: string; body: string }
  | { type: "saveDataView"; id: string; dataView: DataView }
  | { type: "newCrew"; name: string; brief: string; id: string }
  | { type: "member"; name: string }
  | { type: "simulate" | "reset" | "dismissNotice" }
  | { type: "notice"; text: string };
export const scopeNames: Record<Scope, string> = {
  message: "消息",
  todo: "Todo",
  document: "文档/数据",
  progress: "进展",
};
export const statusLabels: Record<Status, string> = {
  open: "待开始",
  active: "进行中",
  blocked: "等待答复",
  done: "已完成",
  dropped: "已结束",
};
